// Declare modules
mod database;
mod doc_loader;
mod embeddings;
mod error;
mod server;

// Use necessary items from modules and crates
use crate::{
    database::Database,
    embeddings::{initialize_embedding_provider, EmbeddingConfig, EMBEDDING_CLIENT},
    error::ServerError,
    server::RustDocsServer,
};
use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use clap::Parser;
use rmcp::{transport::io::stdio, ServiceExt};
use std::env;

use std::collections::HashMap;

#[derive(Parser, Debug)]
#[command(author, version, about = "Rust documentation MCP server using PostgreSQL vector database", long_about = None)]
struct Cli {
    /// The crate names to serve documentation for (space-separated)
    crate_names: Vec<String>,

    /// List all available crates in the database
    #[arg(short, long)]
    list: bool,

    /// Load all available crates from the database
    #[arg(short, long)]
    all: bool,

    /// Embedding provider to use (openai or voyage)
    #[arg(long, default_value = "openai")]
    embedding_provider: String,

    /// Embedding model to use
    #[arg(long)]
    embedding_model: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Parse CLI arguments
    let cli = Cli::parse();

    // Initialize database connection
    eprintln!("🔌 Connecting to database...");
    let db = Database::new().await?;
    eprintln!("✅ Database connected successfully");

    // Handle list command
    if cli.list {
        let stats = db.get_crate_stats().await?;
        if stats.is_empty() {
            println!("No crates found in database.");
            println!("Use the 'populate_db' tool to add crates first:");
            println!("  cargo run --bin populate_db -- <crate_name>");
        } else {
            println!(
                "{:<20} {:<15} {:<10} {:<10} {:<20}",
                "Crate", "Version", "Docs", "Tokens", "Last Updated"
            );
            println!("{:-<80}", "");
            for stat in stats {
                println!(
                    "{:<20} {:<15} {:<10} {:<10} {:<20}",
                    stat.name,
                    stat.version.unwrap_or_else(|| "N/A".to_string()),
                    stat.total_docs,
                    stat.total_tokens,
                    stat.last_updated.format("%Y-%m-%d %H:%M")
                );
            }
        }
        return Ok(());
    }

    // Load crates from database configuration
    eprintln!("Loading crate configurations from database...");
    let crate_configs = db.get_crate_configs(true).await?; // Only enabled crates

    if crate_configs.is_empty() {
        eprintln!("No enabled crates configured in database.");
        eprintln!("Configure crates using the HTTP server's add_crate tool.");
        return Err(ServerError::Config(
            "No crates configured in database.".to_string(),
        ));
    }

    // Determine which crates to load
    let crate_names: Vec<String> = if cli.all {
        eprintln!("Loading all enabled crates from database configuration...");
        crate_configs
            .into_iter()
            .map(|config| config.name)
            .collect()
    } else if cli.crate_names.is_empty() {
        // Default to all enabled crates if none specified
        eprintln!("No crates specified, loading all enabled crates from configuration...");
        crate_configs
            .into_iter()
            .map(|config| config.name)
            .collect()
    } else {
        // Filter to only requested crates that are in the config
        let requested: std::collections::HashSet<_> = cli.crate_names.into_iter().collect();
        crate_configs
            .into_iter()
            .filter(|config| requested.contains(&config.name))
            .map(|config| config.name)
            .collect()
    };

    eprintln!("Target crates: {crate_names:?}");

    // Check if all crates exist in database
    eprintln!("🔍 Checking if crates exist in database...");
    let mut missing_crates = Vec::new();
    for crate_name in &crate_names {
        eprintln!("  Checking: {crate_name}");
        if !db.has_embeddings(crate_name).await? {
            missing_crates.push(crate_name.clone());
            eprintln!("  ❌ Missing: {crate_name}");
        } else {
            eprintln!("  ✅ Found: {crate_name}");
        }
    }

    if !missing_crates.is_empty() {
        eprintln!("Error: The following crates are not found in the database:");
        for crate_name in &missing_crates {
            eprintln!("  - {crate_name}");
        }
        eprintln!("\nPlease populate them first using:");
        for crate_name in &missing_crates {
            eprintln!("  cargo run --bin populate_db -- --crate-name {crate_name}");
        }
        eprintln!("\nOr see available crates with:");
        eprintln!("  cargo run --bin rustdocs_mcp_server -- --list");
        return Err(ServerError::Config(format!(
            "Missing crates: {missing_crates:?}"
        )));
    }

    // Initialize embedding provider (needed for query embedding)
    let provider_name = cli.embedding_provider.to_lowercase();
    eprintln!("🤖 Initializing {provider_name} embedding provider...");

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
    eprintln!("✅ {provider_name} embedding provider initialized");

    // Check database for configured crates
    eprintln!("📋 Checking database for crate configurations...");
    let db_configs = db.get_crate_configs(false).await?;

    if !db_configs.is_empty() {
        eprintln!("  Found {} configured crates in database", db_configs.len());

        // Check if any configured crates need population
        let mut needs_population = Vec::new();
        for config in &db_configs {
            if config.enabled && config.last_populated.is_none() {
                needs_population.push(&config.name);
            }
        }

        if !needs_population.is_empty() {
            eprintln!("\n⚠️  The following crates are configured but not populated:");
            for crate_name in &needs_population {
                eprintln!("  - {crate_name}");
            }
            eprintln!("\n💡 To populate them, run:");
            eprintln!("  cargo run --bin populate_all");
        }
    } else {
        eprintln!("  No crates configured in database yet.");
        eprintln!("  Use the HTTP server's MCP tools or migrate_config to add crates.");
    }

    // Verify crates exist in database (no loading into memory)
    let crate_count = crate_names.len();
    eprintln!("🔍 Verifying {crate_count} crates are available in database...");
    let mut crate_stats = HashMap::new();

    for crate_name in &crate_names {
        let stats = db.get_crate_stats().await?;
        let crate_stat = stats.iter().find(|s| &s.name == crate_name);
        if let Some(stat) = crate_stat {
            crate_stats.insert(crate_name.clone(), stat.total_docs);
            let doc_count = stat.total_docs;
            eprintln!("  ✅ {crate_name}: {doc_count} documents available");
        } else {
            eprintln!("  ❌ {crate_name}: not found in database");
        }
    }

    let total_available_docs: i64 = crate_stats.values().map(|&v| v as i64).sum();

    eprintln!("\n📊 Database Summary:");
    eprintln!("  📚 Total available documents: {total_available_docs}");
    eprintln!("  🗄️  Database-driven search (no memory loading)");

    let startup_message = if crate_names.len() == 1 {
        let doc_count = crate_stats.get(&crate_names[0]).unwrap_or(&0);
        format!(
            "Server for crate '{}' initialized. {} documents available via database search.",
            crate_names[0], doc_count
        )
    } else {
        let crate_summary: Vec<String> = crate_stats
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect();
        format!(
            "Multi-crate server initialized. {} total documents available from {} crates: {}",
            total_available_docs,
            crate_names.len(),
            crate_summary.join(", ")
        )
    };

    eprintln!("\n✅ {startup_message}");

    // Create the service instance (no documents/embeddings in memory)
    let combined_crate_name = if crate_names.len() == 1 {
        crate_names[0].clone()
    } else {
        let crates_joined = crate_names.join(",");
        format!("multi-crate[{crates_joined}]")
    };

    let service = RustDocsServer::new(
        combined_crate_name.clone(),
        vec![], // No documents in memory - use database search
        vec![], // No embeddings in memory - generate on demand
        db,
        startup_message,
    )?;

    eprintln!("Rust Docs MCP server starting via stdio...");

    // Serve the server using stdio transport
    let server_handle = service.serve(stdio()).await.map_err(|e| {
        eprintln!("Failed to start server: {e:?}");
        ServerError::McpRuntime(e.to_string())
    })?;

    eprintln!("Rust Docs MCP server running for: {combined_crate_name}");

    // Wait for the server to complete
    server_handle.waiting().await.map_err(|e| {
        eprintln!("Server encountered an error while running: {e:?}");
        ServerError::McpRuntime(e.to_string())
    })?;

    eprintln!("Rust Docs MCP server stopped.");
    Ok(())
}
