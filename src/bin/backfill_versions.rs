use rustdocs_mcp_server::{database::Database, doc_loader, error::ServerError};
#[tokio::main]
async fn main() -> Result<(), ServerError> {
    dotenvy::dotenv().ok();

    // Initialize database
    let db = Database::new().await?;

    // Get all crates without version
    let crates = db.get_crate_stats().await?;
    let crates_without_version: Vec<_> =
        crates.into_iter().filter(|c| c.version.is_none()).collect();

    println!(
        "Found {} crates without version information",
        crates_without_version.len()
    );

    let mut updated = 0;
    let mut failed = 0;

    for (i, crate_stat) in crates_without_version.iter().enumerate() {
        println!(
            "\n[{}/{}] Processing: {}",
            i + 1,
            crates_without_version.len(),
            crate_stat.name
        );

        // Load just the first page to extract version
        match doc_loader::load_documents_from_docs_rs(&crate_stat.name, "*", None, Some(1)).await {
            Ok(load_result) => {
                if let Some(version) = load_result.version {
                    println!("  ✅ Detected version: {version}");

                    // Update the crate with version
                    match db.upsert_crate(&crate_stat.name, Some(&version)).await {
                        Ok(_) => {
                            println!("  ✅ Updated database");
                            updated += 1;
                        }
                        Err(e) => {
                            println!("  ❌ Failed to update database: {e}");
                            failed += 1;
                        }
                    }
                } else {
                    println!("  ⚠️  No version detected");
                }
            }
            Err(e) => {
                println!("  ❌ Failed to load: {e}");
                failed += 1;
            }
        }

        // Small delay to be respectful to docs.rs
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    println!("\n📊 Summary:");
    println!("  ✅ Updated: {updated} crates");
    println!("  ❌ Failed: {failed} crates");
    println!(
        "  ⚠️  No version: {} crates",
        crates_without_version.len() - updated - failed
    );

    Ok(())
}
