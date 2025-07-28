use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use clap::Parser;
use hyper::{service::service_fn, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use ndarray::Array1;
use rmcp::{
    model::{
        AnnotateAble, CallToolResult, Content, GetPromptRequestParam, GetPromptResult,
        Implementation, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
        PaginatedRequestParam, ProtocolVersion, RawResource, ReadResourceRequestParam,
        ReadResourceResult, Resource, ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer, ServiceExt},
    tool,
    transport::sse_server::{SseServer, SseServerConfig},
    Error as McpError, ServerHandler,
};
use rustdocs_mcp_server::{
    database::Database,
    doc_loader,
    embeddings::{
        generate_embeddings, initialize_embedding_provider, EmbeddingConfig, EMBEDDING_CLIENT,
    },
    error::ServerError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    convert::Infallible,
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Configuration for MCP connection resilience
#[derive(Clone)]
struct McpConnectionConfig {
    /// Timeout for MCP service initialization
    initialize_timeout: Duration,
    /// Maximum number of retry attempts for failed connections
    max_retries: u32,
    /// Base delay for exponential backoff between retries
    retry_base_delay: Duration,
    /// Maximum delay between retries
    #[allow(dead_code)] // Reserved for future retry logic
    retry_max_delay: Duration,
    /// Enable detailed connection logging
    verbose_logging: bool,
}

impl Default for McpConnectionConfig {
    fn default() -> Self {
        Self {
            initialize_timeout: Duration::from_secs(30), // Restored to original value
            max_retries: 3,
            retry_base_delay: Duration::from_millis(500),
            retry_max_delay: Duration::from_secs(10),
            verbose_logging: true,
        }
    }
}

#[derive(Clone)]
struct ReadinessState {
    database_connected: Arc<AtomicBool>,
    embedding_initialized: Arc<AtomicBool>,
    auto_population_complete: Arc<AtomicBool>,
}

impl ReadinessState {
    fn new() -> Self {
        Self {
            database_connected: Arc::new(AtomicBool::new(false)),
            embedding_initialized: Arc::new(AtomicBool::new(false)),
            auto_population_complete: Arc::new(AtomicBool::new(false)),
        }
    }

    fn is_ready(&self) -> bool {
        // Server is ready as soon as database and embeddings are initialized
        // Auto-population can run in background without blocking readiness
        self.database_connected.load(Ordering::Relaxed)
            && self.embedding_initialized.load(Ordering::Relaxed)
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Rust documentation MCP server with HTTP SSE transport", long_about = None)]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value = "3000", env = "PORT")]
    port: u16,

    /// Host to bind to
    #[arg(long, default_value = "0.0.0.0", env = "HOST")]
    host: String,

    /// The crate names to serve documentation for (space-separated)
    #[arg(required = false)]
    crate_names: Vec<String>,

    /// Load all available crates from the database
    #[arg(short, long)]
    all: bool,

    /// Embedding provider to use (openai or voyage)
    #[arg(long, default_value = "openai", env = "EMBEDDING_PROVIDER")]
    embedding_provider: String,

    /// Embedding model to use
    #[arg(long, env = "EMBEDDING_MODEL")]
    embedding_model: Option<String>,
}

#[derive(Clone)]
#[allow(dead_code)] // Fields are used in async trait implementations
struct McpHandler {
    database: Database,
    available_crates: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
    startup_message: String,
}

/// Enhanced MCP connection handler with timeout management and better error handling
async fn handle_mcp_connection_with_resilience(
    handler: McpHandler,
    transport: rmcp::transport::sse_server::SseServerTransport,
    config: McpConnectionConfig,
    connection_id: String,
) -> Result<(), ServerError> {
    let start_time = Instant::now();

    if config.verbose_logging {
        info!("🔄 Starting MCP connection initialization (ID: {connection_id})");
    }

    // Try to establish the connection with extended timeout
    let connection_id_clone = connection_id.clone();
    let connection_result = tokio::time::timeout(config.initialize_timeout, async move {
        match handler.serve(transport).await {
            Ok(service) => {
                if config.verbose_logging {
                    info!("✅ MCP service initialized successfully (ID: {connection_id_clone})");
                }

                // Run the service with enhanced error handling
                info!("🎯 MCP service started successfully (ID: {connection_id_clone})");

                if let Err(e) = service.waiting().await {
                    error!("❌ MCP service runtime error (ID: {connection_id_clone}): {e}");
                    return Err(ServerError::Internal(format!(
                        "MCP service runtime error: {e}"
                    )));
                }

                Ok(())
            }
            Err(e) => {
                if config.verbose_logging {
                    warn!("⚠️  MCP service initialization failed (ID: {connection_id_clone}): {e}");
                }
                Err(ServerError::Internal(format!(
                    "MCP service initialization failed: {e}"
                )))
            }
        }
    })
    .await;

    match connection_result {
        Ok(Ok(())) => {
            // Success!
            let total_time = start_time.elapsed();
            info!("🎉 MCP connection established successfully (ID: {connection_id}, total time: {total_time:?})");
            Ok(())
        }
        Ok(Err(e)) => {
            // Service initialization or runtime error
            let total_time = start_time.elapsed();
            error!("❌ MCP connection failed after {total_time:?} (ID: {connection_id}): {e}");
            Err(e)
        }
        Err(_) => {
            // Timeout
            let total_time = start_time.elapsed();
            error!("⏱️  MCP connection timed out after {total_time:?} (ID: {connection_id})");
            Err(ServerError::Internal(format!(
                "MCP connection timeout after {total_time:?}"
            )))
        }
    }
}

impl McpHandler {
    fn new(database: Database, available_crates: Vec<String>, startup_message: String) -> Self {
        let crates_set: std::collections::HashSet<String> = available_crates.into_iter().collect();
        Self {
            database,
            available_crates: Arc::new(tokio::sync::RwLock::new(crates_set)),
            startup_message,
        }
    }

    /// Refresh the available crates cache from the database
    async fn refresh_available_crates(&self) -> Result<(), ServerError> {
        let all_crates = self.database.get_all_crates_with_embeddings().await?;
        let mut crates = self.available_crates.write().await;
        crates.clear();
        crates.extend(all_crates);
        Ok(())
    }

    /// Add a crate to the available crates cache
    async fn add_crate_to_available(&self, crate_name: &str) {
        let mut crates = self.available_crates.write().await;
        crates.insert(crate_name.to_string());
    }

    /// Check if a crate is available (fast in-memory lookup)
    async fn is_crate_available(&self, crate_name: &str) -> bool {
        let crates = self.available_crates.read().await;
        crates.contains(crate_name)
    }

    /// Remove a crate from the available crates cache
    async fn remove_crate_from_available(&self, crate_name: &str) {
        let mut crates = self.available_crates.write().await;
        crates.remove(crate_name);
    }

    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        RawResource::new(uri, name.to_string()).no_annotation()
    }

    async fn populate_crate(
        &self,
        crate_name: &str,
        features: &[String],
    ) -> Result<serde_json::Value, ServerError> {
        use serde_json::json;

        info!("🚀 Starting automatic population for crate: {}", crate_name);
        let crate_name = crate_name.to_string();
        let features = features.to_vec();
        let database = self.database.clone();

        // Run population in a blocking task to handle non-Send scraper types
        // Use a dedicated thread pool to avoid blocking the main runtime
        let result = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(async {
                let total_start = std::time::Instant::now();

                // Load documents
                info!(
                    "📥 Loading documentation for crate: {} with features: {:?}",
                    crate_name, features
                );
                let doc_start = std::time::Instant::now();
                let features_opt = if features.is_empty() {
                    None
                } else {
                    Some(features.clone())
                };
                let load_result = doc_loader::load_documents_from_docs_rs(
                    &crate_name,
                    "*",
                    features_opt.as_ref(),
                    Some(10000),
                )
                .await?;
                let documents = load_result.documents;
                let crate_version = load_result.version;
                let doc_time = doc_start.elapsed();

                let total_content_size: usize = documents.iter().map(|doc| doc.content.len()).sum();
                info!(
                    "✅ Loaded {} documents in {:.2}s ({:.1} KB total)",
                    documents.len(),
                    doc_time.as_secs_f64(),
                    total_content_size as f64 / 1024.0
                );

                if documents.is_empty() {
                    return Err(ServerError::Config(format!(
                        "No documents found for crate: {crate_name}"
                    )));
                }

                // Generate embeddings
                info!(
                    "🧠 Generating embeddings for {} documents...",
                    documents.len()
                );

                // Yield before heavy embedding operation
                tokio::task::yield_now().await;

                let embedding_start = std::time::Instant::now();
                let (embeddings, total_tokens) = generate_embeddings(&documents).await?;
                let embedding_time = embedding_start.elapsed();

                info!(
                    "✅ Generated {} embeddings using {} tokens in {:.2}s",
                    embeddings.len(),
                    total_tokens,
                    embedding_time.as_secs_f64()
                );

                // Store in database
                info!("💾 Storing embeddings in database...");
                let db_start = std::time::Instant::now();
                let crate_id = database
                    .upsert_crate(&crate_name, crate_version.as_deref())
                    .await?;

                // Initialize tokenizer for accurate token counting
                let bpe =
                    tiktoken_rs::cl100k_base().map_err(|e| ServerError::Tiktoken(e.to_string()))?;

                // Prepare batch data
                let mut batch_data = Vec::new();
                for (path, content, embedding) in embeddings.iter() {
                    let token_count = bpe.encode_with_special_tokens(content).len() as i32;
                    batch_data.push((
                        path.clone(),
                        content.clone(),
                        embedding.clone(),
                        token_count,
                    ));
                }

                database
                    .insert_embeddings_batch(crate_id, &crate_name, &batch_data)
                    .await?;
                let db_time = db_start.elapsed();
                let total_time = total_start.elapsed();

                info!(
                    "🎉 Successfully populated crate {} with {} embeddings in {:.2}s total",
                    crate_name,
                    embeddings.len(),
                    total_time.as_secs_f64()
                );

                Ok(json!({
                    "documents_loaded": documents.len(),
                    "embeddings_generated": embeddings.len(),
                    "total_tokens": total_tokens,
                    "content_size_kb": (total_content_size as f64 / 1024.0).round(),
                    "version": crate_version,
                    "timing": {
                        "doc_loading_secs": doc_time.as_secs_f64(),
                        "embedding_generation_secs": embedding_time.as_secs_f64(),
                        "database_storage_secs": db_time.as_secs_f64(),
                        "total_secs": total_time.as_secs_f64()
                    }
                }))
            })
        })
        .await
        .map_err(|e| ServerError::Internal(format!("Task join error: {e}")))?;

        result
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct QueryRustDocsArgs {
    /// The crate to search in (e.g., "axum", "tokio", "serde")
    crate_name: String,
    /// The specific question about the crate's API or usage.
    question: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct AddCrateArgs {
    /// The crate name (e.g., 'tokio', 'serde')
    crate_name: String,
    /// Version specification: 'latest' or specific version (e.g., '1.35.0')
    version_spec: String,
    /// Optional features to enable (e.g., ['full', 'macros'])
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<Vec<String>>,
    /// Whether the crate is enabled (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    /// Expected number of documents (will be auto-detected if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_docs: Option<i32>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct ListCratesArgs {
    /// Only show enabled crates (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled_only: Option<bool>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct CheckCrateStatusArgs {
    /// The crate name to check status for
    crate_name: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct RemoveCrateArgs {
    /// The crate name to remove
    crate_name: String,
    /// Version specification (default: 'latest')
    #[serde(skip_serializing_if = "Option::is_none")]
    version_spec: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct CrateSpec {
    /// The crate name (e.g., 'tokio', 'serde')
    crate_name: String,
    /// Version specification: 'latest' or specific version (e.g., '1.35.0')
    #[serde(default = "default_version_spec")]
    version_spec: String,
    /// Optional features to enable (e.g., ['full', 'macros'])
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<Vec<String>>,
    /// Whether the crate is enabled (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    /// Expected number of documents (will be auto-detected if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_docs: Option<i32>,
}

fn default_version_spec() -> String {
    "latest".to_string()
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct AddCratesArgs {
    /// List of crates to add/configure
    crates: Vec<CrateSpec>,
    /// Whether to fail fast on first error (default: false - best effort)
    #[serde(skip_serializing_if = "Option::is_none")]
    fail_fast: Option<bool>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct CrateResult {
    /// The crate name
    crate_name: String,
    /// Whether the crate was successfully configured
    success: bool,
    /// Error message if configuration failed
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Status message
    message: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct AddCratesResponse {
    /// Results for each crate
    results: Vec<CrateResult>,
    /// Summary statistics
    summary: AddCratesSummary,
    /// Overall message
    message: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct AddCratesSummary {
    /// Total number of crates processed
    total: usize,
    /// Number of successful configurations
    successful: usize,
    /// Number of failed configurations
    failed: usize,
    /// Number of background ingestion tasks started
    ingestion_started: usize,
}

// Implement ServerHandler trait with correct signatures
#[tool(tool_box)]
impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_logging()
            .build();

        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities,
            server_info: Implementation {
                name: "rustdocs-mcp-server-http".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(self.startup_message.clone()),
        }
    }

    async fn list_resources(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        Err(McpError::invalid_request(
            "No resources available".to_string(),
            None,
        ))
    }

    async fn list_prompts(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            prompts: vec![],
            next_cursor: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let prompt_name = &request.name;
        Err(McpError::invalid_params(
            format!("Prompt not found: {prompt_name}"),
            None,
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![],
            next_cursor: None,
        })
    }
}

// Tool implementation
#[tool(tool_box)]
impl McpHandler {
    #[tool(
        description = "Query documentation for a specific Rust crate using semantic search and LLM summarization."
    )]
    async fn query_rust_docs(
        &self,
        #[tool(aggr)] args: QueryRustDocsArgs,
    ) -> Result<CallToolResult, McpError> {
        // Check if crate is available (fast in-memory lookup)
        if !self.is_crate_available(&args.crate_name).await {
            let crates = self.available_crates.read().await;
            let available_list: Vec<String> = crates.iter().cloned().collect();
            return Err(McpError::invalid_params(
                format!(
                    "Crate '{}' not available. Available crates: {}",
                    args.crate_name,
                    available_list.join(", ")
                ),
                None,
            ));
        }

        // Generate embedding for the question
        let embedding_client = EMBEDDING_CLIENT.get().ok_or_else(|| {
            McpError::internal_error("Embedding client not initialized".to_string(), None)
        })?;

        let (question_embeddings, _) = embedding_client
            .generate_embeddings(&[args.question.clone()])
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Failed to generate embedding: {e}"), None)
            })?;

        let question_embedding = Array1::from_vec(
            question_embeddings
                .first()
                .ok_or_else(|| {
                    McpError::internal_error("No embedding generated".to_string(), None)
                })?
                .clone(),
        );

        // Perform semantic search using the embedding
        match self
            .database
            .search_similar_docs(&args.crate_name, &question_embedding, 10)
            .await
        {
            Ok(results) => {
                if results.is_empty() {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "No relevant documentation found for '{}' in crate '{}'",
                        args.question, args.crate_name
                    ))]))
                } else {
                    // Format search results - results are tuples (id, content, similarity)
                    let crate_name = &args.crate_name;
                    let mut response =
                        format!("From {crate_name} docs (via vector database search): ");

                    // Take top results and format them
                    let formatted_results: Vec<String> = results
                        .into_iter()
                        .take(5) // Limit to top 5 results
                        .enumerate()
                        .map(|(i, (_, content, similarity))| {
                            let idx = i + 1;
                            let content_trimmed = content.trim();
                            format!("{idx}. {content_trimmed} (similarity: {similarity:.3})")
                        })
                        .collect();

                    response.push_str(&formatted_results.join("\n\n"));
                    Ok(CallToolResult::success(vec![Content::text(response)]))
                }
            }
            Err(e) => Err(McpError::internal_error(
                format!("Database search error: {e}"),
                None,
            )),
        }
    }

    #[tool(description = "Add or update a crate configuration")]
    async fn add_crate(
        &self,
        #[tool(aggr)] args: AddCrateArgs,
    ) -> Result<CallToolResult, McpError> {
        use rustdocs_mcp_server::database::CrateConfig;

        info!(
            "🔧 add_crate called for: {} ({})",
            args.crate_name, args.version_spec
        );

        // Validate inputs
        if args.crate_name.is_empty() {
            return Err(McpError::invalid_params("Crate name cannot be empty", None));
        }

        if args.version_spec != "latest" && !args.version_spec.chars().any(|c| c.is_numeric()) {
            return Err(McpError::invalid_params(
                "Version spec must be 'latest' or a valid version number",
                None,
            ));
        }

        // If expected_docs not provided, try to scan for it
        let expected_docs = args.expected_docs.unwrap_or(1000); // Default for now

        // Create config
        let config = CrateConfig {
            id: 0, // Will be set by database
            name: args.crate_name.clone(),
            version_spec: args.version_spec.clone(),
            current_version: None, // Will be set during population
            features: args.features.unwrap_or_default(),
            expected_docs,
            enabled: args.enabled.unwrap_or(true),
            last_checked: None,
            last_populated: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        // Save to database
        match self.database.upsert_crate_config(&config).await {
            Ok(saved_config) => {
                // Create a population job
                let _ = self.database.create_population_job(saved_config.id).await;

                // Return response immediately
                let response = "Ingestion has started".to_string();
                let result = Ok(CallToolResult::success(vec![Content::text(response)]));

                // Spawn background population task after returning response
                let crate_name = args.crate_name.clone();
                let features = saved_config.features.clone();
                let handler_clone = self.clone();
                tokio::spawn(async move {
                    match handler_clone.populate_crate(&crate_name, &features).await {
                        Ok(_) => {
                            // Add the crate to the in-memory cache after successful population
                            handler_clone.add_crate_to_available(&crate_name).await;
                            eprintln!("✅ Background population completed for crate: {crate_name}");
                        }
                        Err(e) => {
                            eprintln!(
                                "⚠️  Background population failed for crate {crate_name}: {e}"
                            );
                        }
                    }
                });

                result
            }
            Err(e) => Err(McpError::internal_error(
                format!("Failed to save crate configuration: {e}"),
                None,
            )),
        }
    }

    #[tool(description = "List all configured crates")]
    async fn list_crates(
        &self,
        #[tool(aggr)] args: ListCratesArgs,
    ) -> Result<CallToolResult, McpError> {
        match self
            .database
            .get_crate_configs(args.enabled_only.unwrap_or(false))
            .await
        {
            Ok(configs) => {
                let crate_list: Vec<serde_json::Value> = configs.iter().map(|config| {
                    serde_json::json!({
                        "name": config.name,
                        "version_spec": config.version_spec,
                        "current_version": config.current_version,
                        "features": config.features,
                        "enabled": config.enabled,
                        "expected_docs": config.expected_docs,
                        "last_populated": config.last_populated,
                        "status": if config.last_populated.is_some() { "populated" } else { "pending" }
                    })
                }).collect();

                let response = serde_json::json!({
                    "crates": crate_list,
                    "total": configs.len()
                });

                Ok(CallToolResult::success(vec![Content::text(
                    response.to_string(),
                )]))
            }
            Err(e) => Err(McpError::internal_error(
                format!("Failed to list crates: {e}"),
                None,
            )),
        }
    }

    #[tool(description = "Check the status of crate population jobs")]
    async fn check_crate_status(
        &self,
        #[tool(aggr)] args: CheckCrateStatusArgs,
    ) -> Result<CallToolResult, McpError> {
        // Get crate configs
        let configs = self.database.get_crate_configs(false).await.map_err(|e| {
            McpError::internal_error(format!("Failed to get crate configs: {e}"), None)
        })?;

        // Find the requested crate
        let config = configs
            .iter()
            .find(|c| c.name == args.crate_name)
            .ok_or_else(|| {
                McpError::invalid_params(format!("Crate '{}' not found", args.crate_name), None)
            })?;

        // Check if crate has embeddings (has been populated)
        let has_embeddings = self
            .database
            .has_embeddings(&args.crate_name)
            .await
            .map_err(|e| {
                McpError::internal_error(format!("Failed to check embeddings: {e}"), None)
            })?;

        // Get document count
        let total_docs = if has_embeddings {
            self.database
                .count_crate_documents(&args.crate_name)
                .await
                .unwrap_or(0) as i32
        } else {
            0
        };

        let status = serde_json::json!({
            "crate_name": config.name,
            "version_spec": config.version_spec,
            "current_version": config.current_version,
            "enabled": config.enabled,
            "last_populated": config.last_populated,
            "has_embeddings": has_embeddings,
            "total_docs": total_docs,
            "features": config.features,
            "expected_docs": config.expected_docs,
            "status": if has_embeddings && total_docs > 0 {
                "populated"
            } else if has_embeddings {
                "empty"
            } else {
                "not_populated"
            },
            "note": if !has_embeddings || total_docs == 0 {
                format!("Run on server: cargo run --bin populate_db -- --crate-name {} --features {}",
                    config.name, config.features.join(" "))
            } else {
                "Crate is populated and ready for queries".to_string()
            }
        });

        Ok(CallToolResult::success(vec![Content::text(
            status.to_string(),
        )]))
    }

    #[tool(description = "Remove a crate configuration")]
    async fn remove_crate(
        &self,
        #[tool(aggr)] args: RemoveCrateArgs,
    ) -> Result<CallToolResult, McpError> {
        let version_spec = args.version_spec.unwrap_or_else(|| "latest".to_string());

        match self
            .database
            .delete_crate_config(&args.crate_name, &version_spec)
            .await
        {
            Ok(deleted) => {
                if deleted {
                    // Remove from in-memory cache
                    self.remove_crate_from_available(&args.crate_name).await;

                    let response = serde_json::json!({
                        "success": true,
                        "message": format!("Removed crate configuration for {} ({})", args.crate_name, version_spec)
                    });
                    Ok(CallToolResult::success(vec![Content::text(
                        response.to_string(),
                    )]))
                } else {
                    Err(McpError::invalid_params(
                        format!(
                            "No configuration found for {} ({})",
                            args.crate_name, version_spec
                        ),
                        None,
                    ))
                }
            }
            Err(e) => Err(McpError::internal_error(
                format!("Failed to remove crate: {e}"),
                None,
            )),
        }
    }

    #[tool(description = "Add or update multiple crate configurations")]
    async fn add_crates(
        &self,
        #[tool(aggr)] args: AddCratesArgs,
    ) -> Result<CallToolResult, McpError> {
        use rustdocs_mcp_server::database::CrateConfig;

        info!("🔧 add_crates called for {} crates", args.crates.len());

        if args.crates.is_empty() {
            return Err(McpError::invalid_params("No crates provided", None));
        }

        let fail_fast = args.fail_fast.unwrap_or(false);
        let mut results = Vec::new();
        let mut successful_count = 0;
        let mut failed_count = 0;
        let mut ingestion_started_count = 0;

        // Process each crate
        for crate_spec in args.crates {
            info!("Processing crate: {}", crate_spec.crate_name);

            // Validate inputs
            let validation_result = self.validate_crate_spec(&crate_spec).await;

            match validation_result {
                Ok(_) => {
                    // Create config
                    let config = CrateConfig {
                        id: 0, // Will be set by database
                        name: crate_spec.crate_name.clone(),
                        version_spec: crate_spec.version_spec.clone(),
                        current_version: None, // Will be set during population
                        features: crate_spec.features.unwrap_or_default(),
                        expected_docs: crate_spec.expected_docs.unwrap_or(1000),
                        enabled: crate_spec.enabled.unwrap_or(true),
                        last_checked: None,
                        last_populated: None,
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };

                    // Save to database
                    match self.database.upsert_crate_config(&config).await {
                        Ok(saved_config) => {
                            // Create a population job
                            let _ = self.database.create_population_job(saved_config.id).await;

                            successful_count += 1;
                            ingestion_started_count += 1;

                            let result = CrateResult {
                                crate_name: crate_spec.crate_name.clone(),
                                success: true,
                                error: None,
                                message: "Configuration saved, ingestion queued".to_string(),
                            };
                            results.push(result);

                            // Spawn background population task
                            let crate_name = crate_spec.crate_name.clone();
                            let features = saved_config.features.clone();
                            let handler_clone = self.clone();
                            tokio::spawn(async move {
                                match handler_clone.populate_crate(&crate_name, &features).await {
                                    Ok(_) => {
                                        // Add the crate to the in-memory cache after successful population
                                        handler_clone.add_crate_to_available(&crate_name).await;
                                        eprintln!("✅ Background population completed for crate: {crate_name}");
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "⚠️  Background population failed for crate {crate_name}: {e}"
                                        );
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            failed_count += 1;
                            let result = CrateResult {
                                crate_name: crate_spec.crate_name.clone(),
                                success: false,
                                error: Some(e.to_string()),
                                message: "Failed to save configuration".to_string(),
                            };
                            results.push(result);

                            if fail_fast {
                                break;
                            }
                        }
                    }
                }
                Err(validation_error) => {
                    failed_count += 1;
                    let result = CrateResult {
                        crate_name: crate_spec.crate_name.clone(),
                        success: false,
                        error: Some(validation_error),
                        message: "Validation failed".to_string(),
                    };
                    results.push(result);

                    if fail_fast {
                        break;
                    }
                }
            }
        }

        // Create response
        let summary = AddCratesSummary {
            total: results.len(),
            successful: successful_count,
            failed: failed_count,
            ingestion_started: ingestion_started_count,
        };

        let message = if failed_count == 0 {
            format!("Successfully configured {successful_count} crates, ingestion started")
        } else if successful_count == 0 {
            format!("Failed to configure any crates ({failed_count} errors)")
        } else {
            format!("Configured {successful_count} crates successfully, {failed_count} failed")
        };

        let response = AddCratesResponse {
            results,
            summary,
            message,
        };

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize response: {e}"), None)
            })?,
        )]))
    }

    // Helper method to validate crate specifications
    async fn validate_crate_spec(&self, crate_spec: &CrateSpec) -> Result<(), String> {
        if crate_spec.crate_name.is_empty() {
            return Err("Crate name cannot be empty".to_string());
        }

        if crate_spec.version_spec != "latest"
            && !crate_spec.version_spec.chars().any(|c| c.is_numeric())
        {
            return Err("Version spec must be 'latest' or a valid version number".to_string());
        }

        // Additional validation can be added here
        Ok(())
    }
}

// Health check handler with liveness and readiness endpoints
fn create_health_handler(
    readiness_state: ReadinessState,
) -> impl Fn(Request<hyper::body::Incoming>) -> Result<Response<String>, Infallible> + Clone {
    move |req: Request<hyper::body::Incoming>| -> Result<Response<String>, Infallible> {
        match (req.method(), req.uri().path()) {
            (&Method::GET, "/health/live") => {
                // Liveness: Just check if the process is alive (always returns OK)
                let response = Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(r#"{"status":"alive","service":"rustdocs-mcp-server"}"#.to_string())
                    .unwrap();
                Ok(response)
            }
            (&Method::GET, "/health/ready") => {
                // Readiness: Check if all initialization is complete
                if readiness_state.is_ready() {
                    let auto_population_complete = readiness_state
                        .auto_population_complete
                        .load(Ordering::Relaxed);
                    let response = Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/json")
                        .body(format!(
                            r#"{{"status":"ready","service":"rustdocs-mcp-server","auto_population_complete":{auto_population_complete}}}"#
                        ))
                        .unwrap();
                    Ok(response)
                } else {
                    let response = Response::builder()
                        .status(StatusCode::SERVICE_UNAVAILABLE)
                        .header("Content-Type", "application/json")
                        .body(format!(
                            r#"{{"status":"not_ready","service":"rustdocs-mcp-server","database_connected":{},"embedding_initialized":{},"auto_population_complete":{}}}"#,
                            readiness_state.database_connected.load(Ordering::Relaxed),
                            readiness_state.embedding_initialized.load(Ordering::Relaxed),
                            readiness_state.auto_population_complete.load(Ordering::Relaxed)
                        ))
                        .unwrap();
                    Ok(response)
                }
            }
            (&Method::GET, "/health") => {
                // Legacy endpoint - redirect to liveness
                let response = Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(r#"{"status":"alive","service":"rustdocs-mcp-server","note":"Use /health/live or /health/ready for specific checks"}"#.to_string())
                    .unwrap();
                Ok(response)
            }
            _ => {
                let response = Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body("Not Found".to_string())
                    .unwrap();
                Ok(response)
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rustdocs_mcp_server_http=info,rmcp=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load .env file if present
    dotenvy::dotenv().ok();

    // Parse CLI arguments
    let cli = Cli::parse();

    let host = &cli.host;
    let port = cli.port;
    info!("🚀 Starting Rust Docs MCP HTTP SSE Server on {host}:{port}");

    // Create readiness state for health checks
    let readiness_state = ReadinessState::new();

    // Start health check server early (before auto-population)
    let health_addr: SocketAddr = format!("{host}:8080")
        .parse()
        .map_err(|e| ServerError::Config(format!("Invalid health bind address: {e}")))?;

    info!("🏥 Starting health server on {health_addr}");
    let health_handler = create_health_handler(readiness_state.clone());
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(health_addr).await.unwrap();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            let handler = health_handler.clone();

            tokio::task::spawn(async move {
                if let Err(err) = Builder::new(TokioExecutor::new())
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let handler = handler.clone();
                            async move { handler(req) }
                        }),
                    )
                    .await
                {
                    tracing::error!("Health server connection error: {}", err);
                }
            });
        }
    });
    info!("✅ Health server started - liveness available at /health/live");

    // Initialize database connection
    info!("🔌 Connecting to database...");
    let db = Database::new().await?;
    readiness_state
        .database_connected
        .store(true, Ordering::Relaxed);
    info!("✅ Database connected successfully");

    // Load crates from database configuration
    info!("Loading crate configurations from database...");
    let crate_configs = db.get_crate_configs(true).await?; // Only enabled crates

    let crate_names: Vec<String> = if crate_configs.is_empty() {
        warn!("No enabled crates configured in database.");
        warn!("Use the 'add_crate' MCP tool to configure crates.");
        warn!("Server will start with no crates available for querying.");
        vec![]
    } else if !cli.crate_names.is_empty() {
        // Filter configs to only those specified on CLI
        crate_configs
            .into_iter()
            .filter(|config| cli.crate_names.contains(&config.name))
            .map(|config| config.name)
            .collect()
    } else {
        // Use all enabled crates from config
        crate_configs
            .into_iter()
            .map(|config| config.name)
            .collect()
    };

    info!("Target crates: {:?}", crate_names);

    // Check if all crates exist in database
    info!("🔍 Checking if crates exist in database...");
    let mut available_crates = Vec::new();
    let mut missing_crates = Vec::new();
    for crate_name in &crate_names {
        if !db.has_embeddings(crate_name).await? {
            missing_crates.push(crate_name.clone());
            warn!("❌ Missing: {crate_name}");
        } else {
            available_crates.push(crate_name.clone());
            info!("✅ Found: {crate_name}");
        }
    }

    // Initialize embedding provider (needed for query embedding and auto-population)
    let provider_name = cli.embedding_provider.to_lowercase();
    info!("🤖 Initializing {provider_name} embedding provider...");

    let embedding_config = match provider_name.as_str() {
        "openai" => {
            let model = cli
                .embedding_model
                .unwrap_or_else(|| "text-embedding-3-large".to_string());
            let openai_client = if let Ok(api_base) = env::var("OPENAI_API_BASE") {
                let config = OpenAIConfig::new().with_api_base(api_base);
                OpenAIClient::with_config(config)
            } else {
                OpenAIClient::new()
            };
            EmbeddingConfig::OpenAI {
                client: openai_client,
                model,
            }
        }
        "voyage" => {
            let api_key = env::var("VOYAGE_API_KEY")
                .map_err(|_| ServerError::MissingEnvVar("VOYAGE_API_KEY".to_string()))?;
            let model = cli
                .embedding_model
                .unwrap_or_else(|| "voyage-3.5".to_string());
            EmbeddingConfig::VoyageAI { api_key, model }
        }
        _ => {
            return Err(ServerError::Config(format!(
                "Unsupported embedding provider: {provider_name}. Use 'openai' or 'voyage'"
            )));
        }
    };

    let provider = initialize_embedding_provider(embedding_config);
    if EMBEDDING_CLIENT.set(provider).is_err() {
        return Err(ServerError::Internal(
            "Failed to set embedding provider".to_string(),
        ));
    }
    readiness_state
        .embedding_initialized
        .store(true, Ordering::Relaxed);
    info!("✅ {provider_name} embedding provider initialized");

    // Note: Auto-population will run after SSE server starts to avoid blocking connections

    // Mark auto-population as complete (whether successful or not)
    readiness_state
        .auto_population_complete
        .store(true, Ordering::Relaxed);
    info!("✅ Auto-population phase complete - service ready");

    // Get crate statistics for startup message (only for available crates)
    let stats = db.get_crate_stats().await?;
    let mut crate_stats = std::collections::HashMap::new();

    for crate_name in &available_crates {
        if let Some(stat) = stats.iter().find(|s| &s.name == crate_name) {
            crate_stats.insert(crate_name.clone(), stat.total_docs);
        }
    }

    let total_docs: i64 = crate_stats.values().map(|&v| v as i64).sum();

    // Create startup message
    let startup_message = if available_crates.is_empty() {
        if missing_crates.is_empty() {
            "HTTP SSE MCP server initialized with no crates. Use the 'add_crate' tool to configure crates.".to_string()
        } else {
            format!(
                "HTTP SSE MCP server initialized. {} crates configured but not populated: {}. Use MCP tools to manage crates.",
                missing_crates.len(),
                missing_crates.join(", ")
            )
        }
    } else if available_crates.len() == 1 {
        let doc_count = crate_stats.get(&available_crates[0]).unwrap_or(&0);
        let missing_note = if !missing_crates.is_empty() {
            format!(
                " (Note: {} crates pending population: {})",
                missing_crates.len(),
                missing_crates.join(", ")
            )
        } else {
            String::new()
        };
        format!(
            "HTTP SSE MCP server for crate '{}' initialized. {} documents available via database search.{}",
            available_crates[0], doc_count, missing_note
        )
    } else {
        let crate_summary: Vec<String> = crate_stats
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect();
        let missing_note = if !missing_crates.is_empty() {
            format!(
                " Note: {} crates pending population: {}",
                missing_crates.len(),
                missing_crates.join(", ")
            )
        } else {
            String::new()
        };
        format!(
            "HTTP SSE MCP multi-crate server initialized. {} total documents available from {} crates: {}.{}",
            total_docs,
            available_crates.len(),
            crate_summary.join(", "),
            missing_note
        )
    };

    info!("✅ {startup_message}");

    // Create the MCP handler with database access (use available crates for queries)
    let handler = McpHandler::new(db.clone(), available_crates, startup_message);

    // Refresh the available crates cache from the database to include any recently added crates
    info!("🔄 Refreshing available crates cache from database...");
    handler.refresh_available_crates().await?;
    info!("✅ Available crates cache refreshed");

    // Create SSE server config
    let host = &cli.host;
    let port = cli.port;
    let bind_addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| ServerError::Config(format!("Invalid bind address: {e}")))?;

    let config = SseServerConfig {
        bind: bind_addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
    };

    info!("🌐 Starting MCP server on {bind_addr}");
    info!("📡 SSE endpoint: http://{bind_addr}/sse");
    info!("📤 POST endpoint: http://{bind_addr}/message");
    info!("🏥 Health endpoints: /health/live (liveness), /health/ready (readiness)");

    // Create and serve SSE server
    let mut sse_server = SseServer::serve_with_config(config)
        .await
        .map_err(|e| ServerError::Internal(format!("Failed to start SSE server: {e}")))?;

    info!("🔧 Server-Sent Events transport ready");
    info!("🎯 MCP server waiting for connections...");

    // Start auto-population in background AFTER server is ready for connections
    if !missing_crates.is_empty() {
        let db_clone = db.clone();
        let missing_crates_clone = missing_crates.clone();
        tokio::spawn(async move {
            info!(
                "🚀 Starting background auto-population for {} missing crates: {:?}",
                missing_crates_clone.len(),
                missing_crates_clone
            );

            // Get crate configurations for missing crates
            match db_clone.get_crate_configs(true).await {
                Ok(all_configs) => {
                    for crate_name in &missing_crates_clone {
                        if let Some(config) = all_configs.iter().find(|c| &c.name == crate_name) {
                            info!(
                                "📦 Auto-populating crate: {} with features: {:?}",
                                config.name, config.features
                            );

                            // Create a temporary handler to use the populate function
                            let temp_handler =
                                McpHandler::new(db_clone.clone(), vec![], String::new());

                            match temp_handler
                                .populate_crate(&config.name, &config.features)
                                .await
                            {
                                Ok(stats) => {
                                    info!("✅ Successfully auto-populated crate: {}", config.name);
                                    info!(
                                        "   📊 Stats: {} documents, {} embeddings",
                                        stats["documents_loaded"], stats["embeddings_generated"]
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "❌ Failed to auto-populate crate: {} - Error: {}",
                                        config.name, e
                                    );
                                }
                            }

                            // Small delay between crate populations to prevent resource starvation
                            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        }
                    }
                    info!("🎉 Background auto-population complete!");
                }
                Err(e) => {
                    warn!("❌ Failed to get crate configs for auto-population: {}", e);
                }
            }
        });
    } else {
        info!("✅ No missing crates - auto-population not needed");
    }

    // Initialize connection configuration with enhanced resilience
    let connection_config = McpConnectionConfig::default();
    info!(
        "⚙️  MCP connection config: timeout={:?}, max_retries={}, retry_delay={:?}",
        connection_config.initialize_timeout,
        connection_config.max_retries,
        connection_config.retry_base_delay
    );

    // Handle incoming transports with enhanced resilience
    let mut connection_counter = 0;
    while let Some(transport) = sse_server.next_transport().await {
        connection_counter += 1;
        let connection_id = format!("conn-{connection_counter}");

        info!("🔗 New MCP connection received (ID: {connection_id})");
        info!("📊 Total active connections: {connection_counter}");

        let handler_clone = handler.clone();
        let config_clone = connection_config.clone();
        let conn_id_clone = connection_id.clone();

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            match handle_mcp_connection_with_resilience(
                handler_clone,
                transport,
                config_clone,
                conn_id_clone.clone(),
            )
            .await
            {
                Ok(()) => {
                    let duration = start_time.elapsed();
                    info!("✅ MCP connection closed gracefully (ID: {conn_id_clone}, duration: {duration:?})");
                }
                Err(e) => {
                    let duration = start_time.elapsed();
                    error!("🚨 MCP connection failed (ID: {conn_id_clone}, duration: {duration:?}): {e}");
                }
            }
        });
    }

    Ok(())
}
