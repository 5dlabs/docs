use async_openai::{config::OpenAIConfig, Client as OpenAIClient};
use futures::future::try_join_all;
use rustdocs_mcp_server::{
    database::Database,
    doc_loader,
    embeddings::{
        generate_embeddings, initialize_embedding_provider, EmbeddingConfig, EMBEDDING_CLIENT,
    },
    error::ServerError,
};
use std::env;

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    dotenvy::dotenv().ok();

    // Initialize database
    println!("📋 Loading crate configurations from database...");
    let db = Database::new().await?;

    // Get enabled crates that need updating
    let crates_to_populate = db.get_crates_needing_update().await?;

    if crates_to_populate.is_empty() {
        println!("✅ All crates are up to date!");
        return Ok(());
    }

    println!(
        "📦 Found {} crates needing update:",
        crates_to_populate.len()
    );
    for config in &crates_to_populate {
        println!(
            "  - {} ({}) {:?}",
            config.name, config.version_spec, config.features
        );
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
            let model = env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "voyage-3.5".to_string());
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

    let _embedding_model =
        env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "text-embedding-3-small".to_string());

    println!(
        "\n🚀 Starting parallel population of {} crates...",
        crates_to_populate.len()
    );
    let start_time = std::time::Instant::now();

    // Create tasks for parallel processing
    let tasks: Vec<_> = crates_to_populate
        .into_iter()
        .enumerate()
        .map(|(i, crate_config)| {
            let db = &db;
            let crate_name = crate_config.name.clone();
            let features = crate_config.features.clone();
            let config_id = crate_config.id;

            async move {
                println!(
                    "\n📥 [{}/{}] Loading documentation for: {}",
                    i + 1,
                    i + 1,
                    crate_name
                );

                // Create population job
                let job_id = db.create_population_job(config_id).await?;
                db.update_population_job(job_id, "running", None, None)
                    .await?;

                let doc_start = std::time::Instant::now();

                let result = match doc_loader::load_documents_from_docs_rs(
                    &crate_name,
                    "*",
                    Some(&features),
                    Some(50), // Use smaller page limit for batch processing
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => {
                        println!("❌ Failed to populate {crate_name}: {e}");
                        let error_msg = e.to_string();
                        db.update_population_job(job_id, "failed", Some(&error_msg), None)
                            .await?;
                        return Err(ServerError::DocLoader(e));
                    }
                };

                let documents = result.documents;
                let crate_version = result.version;

                let doc_time = doc_start.elapsed();
                println!(
                    "✅ [{}/{}] Loaded {} documents for {} in {:.2}s",
                    i + 1,
                    i + 1,
                    documents.len(),
                    crate_name,
                    doc_time.as_secs_f64()
                );

                if let Some(ref version) = crate_version {
                    println!(
                        "📦 [{}/{}] Detected version for {}: {}",
                        i + 1,
                        i + 1,
                        crate_name,
                        version
                    );
                }

                if documents.is_empty() {
                    println!("⚠️  No documents found for {crate_name}");
                    db.update_population_job(job_id, "completed", None, Some(0))
                        .await?;
                    return Ok::<_, ServerError>((crate_name, 0, 0.0));
                }

                // Generate embeddings
                println!(
                    "🧠 [{}/{}] Generating embeddings for {}...",
                    i + 1,
                    i + 1,
                    crate_name
                );
                let embed_start = std::time::Instant::now();
                let (embeddings, total_tokens) = generate_embeddings(&documents).await?;
                let embed_time = embed_start.elapsed();

                let cost_per_million = 0.02;
                let estimated_cost = (total_tokens as f64 / 1_000_000.0) * cost_per_million;
                println!(
                    "✅ [{}/{}] Generated {} embeddings for {} in {:.2}s (${:.6})",
                    i + 1,
                    i + 1,
                    embeddings.len(),
                    crate_name,
                    embed_time.as_secs_f64(),
                    estimated_cost
                );

                // Store in database
                let crate_id = db
                    .upsert_crate(&crate_name, crate_version.as_deref())
                    .await?;

                // Initialize tokenizer for accurate token counting
                let bpe =
                    tiktoken_rs::cl100k_base().map_err(|e| ServerError::Tiktoken(e.to_string()))?;

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

                // Update crate config with current version and last populated time
                let mut updated_config = crate_config.clone();
                updated_config.current_version = crate_version;
                updated_config.last_populated = Some(chrono::Utc::now());
                updated_config.last_checked = Some(chrono::Utc::now());
                db.upsert_crate_config(&updated_config).await?;

                // Mark job as completed
                db.update_population_job(job_id, "completed", None, Some(embeddings.len() as i32))
                    .await?;

                // Add delay between crates to be respectful to docs.rs
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                Ok((crate_name, embeddings.len(), estimated_cost))
            }
        })
        .collect();

    // Execute all tasks in parallel
    let results = try_join_all(tasks).await?;
    let total_time = start_time.elapsed();

    // Summary
    println!(
        "\n🎉 Population complete! Total time: {:.2}s",
        total_time.as_secs_f64()
    );
    println!("📊 Summary:");

    let mut total_embeddings = 0;
    let mut total_cost = 0.0;

    for (crate_name, embedding_count, cost) in results {
        println!("  ✅ {crate_name}: {embedding_count} embeddings (${cost:.6})");
        total_embeddings += embedding_count;
        total_cost += cost;
    }

    println!("\n📈 Total: {total_embeddings} embeddings");
    println!("💰 Total estimated cost: ${total_cost:.6}");

    Ok(())
}
