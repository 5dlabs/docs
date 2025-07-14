use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use clap::Parser;
use rustdocs_mcp_server::{
    database::Database,
    doc_loader,
    embeddings::{
        generate_embeddings, initialize_embedding_provider, EmbeddingConfig, EMBEDDING_CLIENT,
    },
    error::ServerError,
};
use std::env;

#[derive(Parser, Debug)]
#[command(author, version, about = "Populate Rust docs database with embeddings", long_about = None)]
struct Cli {
    /// The crate name to populate (e.g., "tokio", "serde")
    #[arg(short, long)]
    crate_name: Option<String>,

    /// List all crates in the database
    #[arg(short, long)]
    list: bool,

    /// Delete embeddings for a specific crate
    #[arg(short, long)]
    delete: Option<String>,

    /// Force regeneration even if embeddings exist
    #[arg(short, long)]
    force: bool,

    /// Test mode - only load docs, don't generate embeddings
    #[arg(short, long)]
    test: bool,

    /// Optional features to enable for the crate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 0..)]
    features: Option<Vec<String>>,

    /// Maximum number of pages to crawl (default: 10000)
    #[arg(long, default_value_t = 10000)]
    max_pages: usize,
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Initialize database
    let db = Database::new().await?;

    // Handle list command
    if cli.list {
        let stats = db.get_crate_stats().await?;
        if stats.is_empty() {
            println!("No crates in database.");
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

    // Handle delete command
    if let Some(crate_to_delete) = cli.delete {
        println!("Deleting embeddings for crate: {crate_to_delete}");
        db.delete_crate_embeddings(&crate_to_delete).await?;
        println!("Successfully deleted embeddings for {crate_to_delete}");
        return Ok(());
    }

    // Handle populate command
    if let Some(crate_name) = cli.crate_name {
        // Check if embeddings already exist
        if !cli.force && db.has_embeddings(&crate_name).await? {
            println!("Embeddings already exist for {crate_name}. Use --force to regenerate.");
            return Ok(());
        }

        // Initialize embedding provider (default to OpenAI for populate script)
        let provider_type = env::var("EMBEDDING_PROVIDER").unwrap_or_else(|_| "openai".to_string());
        let embedding_config = match provider_type.to_lowercase().as_str() {
            "openai" => {
                let model = env::var("EMBEDDING_MODEL")
                    .unwrap_or_else(|_| "text-embedding-3-large".to_string());
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
                let model =
                    env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "voyage-3.5".to_string());
                EmbeddingConfig::VoyageAI { api_key, model }
            }
            _ => {
                return Err(ServerError::Config(format!(
                    "Unsupported embedding provider: {provider_type}. Use 'openai' or 'voyage'"
                )));
            }
        };

        let provider = initialize_embedding_provider(embedding_config);
        if EMBEDDING_CLIENT.set(provider).is_err() {
            return Err(ServerError::Internal(
                "Failed to set embedding provider".to_string(),
            ));
        }

        // Initialize tokenizer for accurate token counting
        let bpe = tiktoken_rs::cl100k_base().map_err(|e| ServerError::Tiktoken(e.to_string()))?;

        println!(
            "📥 Loading documentation for crate: {crate_name} (max {} pages)",
            cli.max_pages
        );
        let doc_start = std::time::Instant::now();
        let load_result = doc_loader::load_documents_from_docs_rs(
            &crate_name,
            "*",
            cli.features.as_ref(),
            Some(cli.max_pages),
        )
        .await?;
        let documents = load_result.documents;
        let crate_version = load_result.version;
        let doc_time = doc_start.elapsed();

        let total_content_size: usize = documents.iter().map(|doc| doc.content.len()).sum();
        println!(
            "✅ Loaded {} documents in {:.2}s ({:.1} KB total)",
            documents.len(),
            doc_time.as_secs_f64(),
            total_content_size as f64 / 1024.0
        );

        if let Some(ref version) = crate_version {
            println!("📦 Detected version: {version}");
        }

        if documents.is_empty() {
            println!("No documents found for crate: {crate_name}");
            return Ok(());
        }

        // If test mode, just show what we loaded and exit
        if cli.test {
            println!("\n🧪 Test mode - showing loaded documents:");
            for (i, doc) in documents.iter().enumerate() {
                println!(
                    "  📄 {}: {} ({:.1} KB)",
                    i + 1,
                    doc.path,
                    doc.content.len() as f64 / 1024.0
                );
                if i < 3 {
                    // Show first few documents
                    println!(
                        "     Preview: {}...",
                        doc.content
                            .chars()
                            .take(100)
                            .collect::<String>()
                            .replace('\n', " ")
                    );
                }
            }
            println!(
                "\n📊 Summary: {} documents, {:.1} KB total content",
                documents.len(),
                total_content_size as f64 / 1024.0
            );
            return Ok(());
        }

        // Generate embeddings
        println!("\n🧠 Generating embeddings...");
        let embedding_start = std::time::Instant::now();
        let (embeddings, total_tokens) = generate_embeddings(&documents).await?;
        let embedding_time = embedding_start.elapsed();

        let cost_per_million = 0.02;
        let estimated_cost = (total_tokens as f64 / 1_000_000.0) * cost_per_million;
        println!(
            "✅ Generated {} embeddings using {} tokens in {:.2}s (Est. Cost: ${:.6})",
            embeddings.len(),
            total_tokens,
            embedding_time.as_secs_f64(),
            estimated_cost
        );

        // Insert into database
        println!("\n💾 Storing in database...");
        let db_start = std::time::Instant::now();
        let crate_id = db
            .upsert_crate(&crate_name, crate_version.as_deref())
            .await?;

        // Prepare batch data
        let mut batch_data = Vec::new();
        for (path, content, embedding) in embeddings.iter() {
            // Calculate actual token count for this chunk
            let token_count = bpe.encode_with_special_tokens(content).len() as i32;
            batch_data.push((
                path.clone(),
                content.clone(),
                embedding.clone(),
                token_count,
            ));
        }

        db.insert_embeddings_batch(crate_id, &crate_name, &batch_data)
            .await?;
        let db_time = db_start.elapsed();
        let total_time = doc_start.elapsed();

        println!(
            "✅ Successfully stored {} embeddings for {crate_name} in {:.2}s",
            embeddings.len(),
            db_time.as_secs_f64()
        );

        println!(
            "\n🎉 Complete! Total time: {:.2}s",
            total_time.as_secs_f64()
        );
        println!("📊 Final Summary:");
        println!("  📥 Document loading: {:.2}s", doc_time.as_secs_f64());
        println!(
            "  🧠 Embedding generation: {:.2}s",
            embedding_time.as_secs_f64()
        );
        println!("  💾 Database storage: {:.2}s", db_time.as_secs_f64());
        println!("  💰 Estimated cost: ${estimated_cost:.6}");
    } else {
        println!(
            "Please specify a crate name with --crate-name or use --list to see existing crates"
        );
    }

    Ok(())
}
