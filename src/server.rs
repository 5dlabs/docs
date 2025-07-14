use crate::{
    database::Database,
    doc_loader::Document,
    embeddings::EMBEDDING_CLIENT,
    error::ServerError, // Keep ServerError for ::new()
};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
    Client as OpenAIClient,
};
use ndarray::Array1;
use rmcp::model::AnnotateAble; // Import trait for .no_annotation()
use rmcp::{
    model::{
        CallToolResult,
        Content,
        GetPromptRequestParam,
        GetPromptResult,
        /* EmptyObject, ErrorCode, */ Implementation,
        ListPromptsResult, // Removed EmptyObject, ErrorCode
        ListResourceTemplatesResult,
        ListResourcesResult,
        LoggingLevel, // Uncommented ListToolsResult
        LoggingMessageNotification,
        LoggingMessageNotificationMethod,
        LoggingMessageNotificationParam,
        Notification,
        PaginatedRequestParam,
        ProtocolVersion,
        RawResource,
        /* Prompt, PromptArgument, PromptMessage, PromptMessageContent, PromptMessageRole, */ // Removed Prompt types
        ReadResourceRequestParam,
        ReadResourceResult,
        Resource,
        ResourceContents,
        ServerCapabilities,
        ServerInfo,
        ServerNotification,
    },
    service::{RequestContext, RoleServer},
    tool,
    Error as McpError,
    Peer,
    ServerHandler, // Import necessary rmcp items
};
use schemars::JsonSchema; // Import JsonSchema
use serde::Deserialize; // Import Deserialize
use serde_json::json;
use std::{/* borrow::Cow, */ env, sync::Arc}; // Removed borrow::Cow
use tokio::sync::Mutex;

// --- Argument Struct for the Tool ---

#[derive(Debug, Deserialize, JsonSchema)]
struct QueryRustDocsArgs {
    #[schemars(description = "The crate to search in (e.g., \"axum\", \"tokio\", \"serde\")")]
    crate_name: String,
    #[schemars(description = "The specific question about the crate's API or usage.")]
    question: String,
}

// --- Main Server Struct ---

// No longer needs ServerState, holds data directly
#[derive(Clone)] // Add Clone for tool macro requirements
pub struct RustDocsServer {
    crate_name: Arc<String>, // Use Arc for cheap cloning
    embeddings: Arc<Vec<(String, Array1<f32>)>>,
    database: Arc<Database>,                     // Add database connection
    peer: Arc<Mutex<Option<Peer<RoleServer>>>>,  // Uses tokio::sync::Mutex
    startup_message: Arc<Mutex<Option<String>>>, // Keep the message itself
    startup_message_sent: Arc<Mutex<bool>>,      // Flag to track if sent (using tokio::sync::Mutex)
                                                 // tool_name and info are handled by ServerHandler/macros now
}

impl RustDocsServer {
    // Updated constructor
    pub fn new(
        crate_name: String,
        _documents: Vec<Document>,
        embeddings: Vec<(String, Array1<f32>)>,
        database: Database,
        startup_message: String,
    ) -> Result<Self, ServerError> {
        // Keep ServerError for potential future init errors
        Ok(Self {
            crate_name: Arc::new(crate_name),
            embeddings: Arc::new(embeddings),
            database: Arc::new(database),
            peer: Arc::new(Mutex::new(None)), // Uses tokio::sync::Mutex
            startup_message: Arc::new(Mutex::new(Some(startup_message))), // Initialize message
            startup_message_sent: Arc::new(Mutex::new(false)), // Initialize flag to false
        })
    }

    // Helper function to send log messages via MCP notification (remains mostly the same)
    pub fn send_log(&self, level: LoggingLevel, message: String) {
        let peer_arc = Arc::clone(&self.peer);
        tokio::spawn(async move {
            let mut peer_guard = peer_arc.lock().await;
            if let Some(peer) = peer_guard.as_mut() {
                let params = LoggingMessageNotificationParam {
                    level,
                    logger: None,
                    data: serde_json::Value::String(message),
                };
                let log_notification: LoggingMessageNotification = Notification {
                    method: LoggingMessageNotificationMethod,
                    params,
                };
                let server_notification =
                    ServerNotification::LoggingMessageNotification(log_notification);
                if let Err(e) = peer.send_notification(server_notification).await {
                    eprintln!("Failed to send MCP log notification: {e}");
                }
            } else {
                eprintln!("Log task ran but MCP peer was not connected.");
            }
        });
    }

    // Helper for creating simple text resources (like in counter example)
    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        RawResource::new(uri, name.to_string()).no_annotation()
    }
}

// --- Tool Implementation ---

#[tool(tool_box)] // Add tool_box here as well, mirroring the example
                  // Tool methods go in a regular impl block
impl RustDocsServer {
    // Define the tool using the tool macro
    // Name removed; will be handled dynamically by overriding list_tools/get_tool
    #[tool(
        description = "Query documentation for a specific Rust crate using semantic search and LLM summarization."
    )]
    async fn query_rust_docs(
        &self,
        #[tool(aggr)] // Aggregate arguments into the struct
        args: QueryRustDocsArgs,
    ) -> Result<CallToolResult, McpError> {
        // --- Send Startup Message (if not already sent) ---
        let mut sent_guard = self.startup_message_sent.lock().await;
        if !*sent_guard {
            let mut msg_guard = self.startup_message.lock().await;
            if let Some(message) = msg_guard.take() {
                // Take the message out
                self.send_log(LoggingLevel::Info, message);
                *sent_guard = true; // Mark as sent
            }
            // Drop guards explicitly to avoid holding locks longer than needed
            drop(msg_guard);
            drop(sent_guard);
        } else {
            // Drop guard if already sent
            drop(sent_guard);
        }

        let crate_name = &args.crate_name;
        let question = &args.question;

        // Use the explicitly provided crate name
        let target_crate = crate_name;

        // Log received query via MCP
        self.send_log(
            LoggingLevel::Info,
            format!("Searching in crate '{target_crate}' for: {question}"),
        );

        // --- Embedding Generation for Question ---
        let embedding_provider = EMBEDDING_CLIENT
            .get()
            .ok_or_else(|| McpError::internal_error("Embedding provider not initialized", None))?;

        // Generate embedding for the question using the configured provider
        let (embeddings, _tokens) = embedding_provider
            .generate_embeddings(&[question.to_string()])
            .await
            .map_err(|e| McpError::internal_error(format!("Embedding API error: {e}"), None))?;

        let question_embedding = embeddings.into_iter().next().ok_or_else(|| {
            McpError::internal_error("Failed to get embedding for question", None)
        })?;

        let question_vector = Array1::from(question_embedding);

        // --- Search for similar documents using database ---
        self.send_log(
            LoggingLevel::Info,
            format!("Performing vector search in database for crate '{target_crate}'"),
        );

        let search_results = self
            .database
            .search_similar_docs(target_crate, &question_vector, 3)
            .await
            .map_err(|e| {
                self.send_log(LoggingLevel::Error, format!("Database search failed: {e}"));
                McpError::internal_error(format!("Database search error: {e}"), None)
            })?;

        // --- Generate Response using LLM ---
        let response_text = if !search_results.is_empty() {
            let (best_path, best_content, best_score) = &search_results[0];

            self.send_log(
                LoggingLevel::Info,
                format!(
                    "Found {} relevant documents via vector DB. Best match: {best_path} (similarity: {best_score:.3})",
                    search_results.len()
                ),
            );

            // Combine top results for better context
            let combined_context = if search_results.len() > 1 {
                search_results
                    .iter()
                    .enumerate()
                    .map(|(i, (path, content, score))| {
                        format!(
                            "--- Document {} (similarity: {score:.3}) ---\nPath: {path}\n\n{content}",
                            i + 1
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            } else {
                best_content.clone()
            };

            // Check if this is an in-memory fallback or actual DB result
            let source = if self.embeddings.is_empty() {
                "vector database"
            } else {
                "vector database (with in-memory cache)"
            };

            let result_count = search_results.len();
            self.send_log(
                LoggingLevel::Info,
                format!("Using {result_count} results from {source} for LLM context"),
            );

            {
                // Get OpenAI client for LLM chat completion (separate from embedding provider)
                let openai_client = if let Ok(api_base) = env::var("OPENAI_API_BASE") {
                    let config = OpenAIConfig::new().with_api_base(api_base);
                    OpenAIClient::with_config(config)
                } else {
                    OpenAIClient::new()
                };

                let system_prompt = format!(
                        "You are an expert technical assistant for the Rust crate '{target_crate}'. \
                         Answer the user's question based *only* on the provided context. \
                         If the context does not contain the answer, say so. \
                         Do not make up information. Be clear, concise, and comprehensive providing example usage code when possible."
                    );
                let user_prompt =
                    format!("Context:\n---\n{combined_context}\n---\n\nQuestion: {question}");

                let llm_model: String =
                    env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini-2024-07-18".to_string());
                let chat_request = CreateChatCompletionRequestArgs::default()
                    .model(llm_model)
                    .messages(vec![
                        ChatCompletionRequestSystemMessageArgs::default()
                            .content(system_prompt)
                            .build()
                            .map_err(|e| {
                                McpError::internal_error(
                                    format!("Failed to build system message: {e}"),
                                    None,
                                )
                            })?
                            .into(),
                        ChatCompletionRequestUserMessageArgs::default()
                            .content(user_prompt)
                            .build()
                            .map_err(|e| {
                                McpError::internal_error(
                                    format!("Failed to build user message: {e}"),
                                    None,
                                )
                            })?
                            .into(),
                    ])
                    .build()
                    .map_err(|e| {
                        McpError::internal_error(format!("Failed to build chat request: {e}"), None)
                    })?;

                let chat_response =
                    openai_client
                        .chat()
                        .create(chat_request)
                        .await
                        .map_err(|e| {
                            McpError::internal_error(format!("OpenAI chat API error: {e}"), None)
                        })?;

                self.send_log(
                    LoggingLevel::Info,
                    "Generating response using LLM based on vector DB results".to_string(),
                );

                chat_response
                    .choices
                    .first()
                    .and_then(|choice| choice.message.content.clone())
                    .unwrap_or_else(|| "Error: No response from LLM.".to_string())
            }
        } else {
            self.send_log(
                LoggingLevel::Warning,
                format!("No relevant documents found in vector DB for crate '{target_crate}'"),
            );
            "No relevant documentation found in the vector database for this query.".to_string()
        };

        // --- Format and Return Result ---
        let final_response = if !search_results.is_empty() {
            format!("From {target_crate} docs (via vector database search): {response_text}")
        } else {
            format!("From {target_crate} docs: {response_text}")
        };

        self.send_log(
            LoggingLevel::Info,
            "Successfully generated response".to_string(),
        );

        Ok(CallToolResult::success(vec![Content::text(final_response)]))
    }
}

// --- ServerHandler Implementation ---

#[tool(tool_box)] // Use imported tool macro directly
impl ServerHandler for RustDocsServer {
    fn get_info(&self) -> ServerInfo {
        // Define capabilities using the builder
        let capabilities = ServerCapabilities::builder()
            .enable_tools() // Enable tools capability
            .enable_logging() // Enable logging capability
            // Add other capabilities like resources, prompts if needed later
            .build();

        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05, // Use latest known version
            capabilities,
            server_info: Implementation {
                name: "rust-docs-mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            // Provide instructions based on the specific crate
            instructions: Some(format!(
                "This server provides tools to query documentation for the '{}' crate. \
                 Use the 'query_rust_docs' tool with a specific question to get information \
                 about its API, usage, and examples, derived from its official documentation.",
                self.crate_name
            )),
        }
    }

    // --- Placeholder Implementations for other ServerHandler methods ---
    // Implement these properly if resource/prompt features are added later.

    async fn list_resources(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // Example: Return the crate name as a resource
        Ok(ListResourcesResult {
            resources: vec![self._create_resource_text(
                &format!("crate://{crate_name}", crate_name = self.crate_name),
                "crate_name",
            )],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let expected_uri = format!("crate://{crate_name}", crate_name = self.crate_name);
        if request.uri == expected_uri {
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(
                    self.crate_name.as_str(), // Explicitly get &str from Arc<String>
                    &request.uri,
                )],
            })
        } else {
            Err(McpError::resource_not_found(
                format!("Resource URI not found: {uri}", uri = request.uri),
                Some(json!({ "uri": request.uri })),
            ))
        }
    }

    async fn list_prompts(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            next_cursor: None,
            prompts: Vec::new(), // No prompts defined yet
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        Err(McpError::invalid_params(
            // Or prompt_not_found if that exists
            format!("Prompt not found: {name}", name = request.name),
            None,
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(), // No templates defined yet
        })
    }
}
