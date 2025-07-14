use ndarray::Array1;
use rustdocs_mcp_server::{database::Database, embeddings::*};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment
    dotenvy::dotenv().ok();

    // Connect to database
    println!("Connecting to database...");
    let db = Database::new().await?;

    // Initialize embedding provider
    println!("Initializing embedding provider...");
    let openai_client = if let Ok(api_base) = env::var("OPENAI_API_BASE") {
        let config = async_openai::config::OpenAIConfig::new().with_api_base(api_base);
        async_openai::Client::with_config(config)
    } else {
        async_openai::Client::new()
    };

    let embedding_config = EmbeddingConfig::OpenAI {
        client: openai_client,
        model: "text-embedding-ada-002".to_string(),
    };

    let provider = initialize_embedding_provider(embedding_config);

    // Test question
    let question =
        "How do I create routes in axum and what are the different ways to define route handlers?";
    println!("\nQuestion: {question}");

    // Generate embedding
    println!("Generating embedding...");
    let (embeddings, _) = provider
        .generate_embeddings(&[question.to_string()])
        .await?;
    let query_embedding = Array1::from(embeddings[0].clone());

    // Search in database
    println!("Searching in database for crate 'axum'...");
    let results = db.search_similar_docs("axum", &query_embedding, 5).await?;

    println!("\nFound {} results:", results.len());
    for (i, (path, content, similarity)) in results.iter().enumerate() {
        println!("\n--- Result {} (similarity: {:.3}) ---", i + 1, similarity);
        println!("Path: {path}");
        println!("Content preview: {}...", &content[..content.len().min(200)]);
    }

    Ok(())
}
