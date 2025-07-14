use rustdocs_mcp_server::{
    database::{CrateConfig, Database},
    error::ServerError,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
struct ProxyConfig {
    rustdocs_binary_path: String,
    crates: Vec<OldCrateConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OldCrateConfig {
    name: String,
    features: Option<Vec<String>>,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_docs: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    dotenvy::dotenv().ok();

    // Check if proxy-config.json exists
    if !Path::new("proxy-config.json").exists() {
        println!("No proxy-config.json found. Nothing to migrate.");
        return Ok(());
    }

    // Read proxy-config.json
    println!("📋 Reading proxy-config.json...");
    let config_content = fs::read_to_string("proxy-config.json")
        .map_err(|e| ServerError::Config(format!("Failed to read proxy-config.json: {e}")))?;

    let config: ProxyConfig = serde_json::from_str(&config_content)
        .map_err(|e| ServerError::Config(format!("Failed to parse proxy-config.json: {e}")))?;

    println!("Found {} crates in proxy-config.json", config.crates.len());

    // Initialize database
    let db = Database::new().await?;

    // Migrate each crate
    let mut migrated = 0;
    let mut skipped = 0;

    for old_config in config.crates {
        println!(
            "\nMigrating: {} (enabled: {})",
            old_config.name, old_config.enabled
        );

        // Check if already exists
        if let Some(existing) = db.get_crate_config(&old_config.name, "latest").await? {
            println!(
                "  ⚠️  Already exists in database (id: {}), skipping",
                existing.id
            );
            skipped += 1;
            continue;
        }

        // Create new config
        let new_config = CrateConfig {
            id: 0, // Will be set by database
            name: old_config.name.clone(),
            version_spec: "latest".to_string(),
            current_version: None,
            features: old_config.features.unwrap_or_default(),
            expected_docs: old_config.expected_docs.unwrap_or(1000) as i32,
            enabled: old_config.enabled,
            last_checked: None,
            last_populated: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        match db.upsert_crate_config(&new_config).await {
            Ok(saved) => {
                println!("  ✅ Migrated successfully (id: {})", saved.id);
                migrated += 1;
            }
            Err(e) => {
                println!("  ❌ Failed to migrate: {e}");
            }
        }
    }

    println!("\n📊 Migration Summary:");
    println!("  ✅ Migrated: {migrated} crates");
    println!("  ⚠️  Skipped: {skipped} crates (already existed)");

    // Offer to rename the old config
    if migrated > 0 {
        println!("\n💡 Migration complete! You can now:");
        println!("  1. Rename proxy-config.json to proxy-config.json.bak");
        println!("  2. Use the 'add_crate' and 'list_crates' MCP tools to manage crates");
        println!("  3. Run 'populate_all' to populate any missing documentation");
    }

    Ok(())
}
