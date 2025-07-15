use clap::Parser;
use rustdocs_mcp_server::{database::Database, error::ServerError};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about = "Add a crate to proxy-config.json with expected document count", long_about = None)]
struct Cli {
    /// The crate name to add
    crate_name: String,

    /// Optional features to enable for the crate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 0..)]
    features: Option<Vec<String>>,

    /// Maximum pages to scan for document counting (default: 500)
    #[arg(long, default_value_t = 500)]
    max_scan_pages: usize,

    /// Enable the crate (default: true)
    #[arg(long, default_value_t = true)]
    enabled: bool,

    /// Force update if crate already exists
    #[arg(short, long)]
    force: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProxyConfig {
    rustdocs_binary_path: String,
    crates: Vec<CrateConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CrateConfig {
    name: String,
    features: Option<Vec<String>>,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_docs: Option<usize>,
}

async fn scan_crate_docs_count(crate_name: &str, max_pages: usize) -> Result<usize, ServerError> {
    println!("🔍 Scanning docs.rs to estimate document count for: {crate_name}");

    let base_url = format!("https://docs.rs/{crate_name}/latest/{crate_name}/");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| ServerError::Network(e.to_string()))?;

    let mut visited = HashSet::new();
    let mut to_visit = VecDeque::new();
    to_visit.push_back(base_url.clone());

    let mut doc_pages_found = 0;
    let mut processed = 0;

    // More selective selectors for pages with substantial documentation content
    let content_selectors = vec![Selector::parse("div.docblock, section.docblock")
        .map_err(|e| ServerError::Internal(format!("CSS selector error: {e}")))?];

    // Additional selector for pages that have implementation details
    let secondary_selectors = vec![
        Selector::parse(".impl-items")
            .map_err(|e| ServerError::Internal(format!("CSS selector error: {e}")))?,
        Selector::parse(".item-info .stab")
            .map_err(|e| ServerError::Internal(format!("CSS selector error: {e}")))?,
    ];

    while let Some(url) = to_visit.pop_front() {
        if processed >= max_pages {
            println!(
                "⚠️  Reached scan limit of {max_pages} pages, found {doc_pages_found} docs so far"
            );
            break;
        }

        if visited.contains(&url) {
            continue;
        }

        visited.insert(url.clone());
        processed += 1;

        if processed % 50 == 0 {
            println!("📊 Scanned {processed}/{max_pages} pages, found {doc_pages_found} docs");
        }

        let html_content = match fetch_with_retry(&client, &url, 3).await {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Failed to fetch {url} after retries: {e}");
                continue;
            }
        };

        let document = Html::parse_document(&html_content);

        // Check if this page has substantial documentation content
        let mut has_primary_content = false;
        let mut has_secondary_content = false;

        // Check for primary documentation content (docblocks)
        for selector in &content_selectors {
            if document.select(selector).next().is_some() {
                has_primary_content = true;
                break;
            }
        }

        // Only check secondary content if no primary content found
        if !has_primary_content {
            for selector in &secondary_selectors {
                if document.select(selector).next().is_some() {
                    has_secondary_content = true;
                    break;
                }
            }
        }

        // Count page if it has primary content, or if it's a meaningful secondary page
        if has_primary_content
            || (has_secondary_content && !url.contains("index.html") && !url.contains("all.html"))
        {
            doc_pages_found += 1;
        }

        // Find new links to follow (only within the same crate docs)
        if let Ok(link_selector) = Selector::parse("a[href]") {
            for link in document.select(&link_selector) {
                if let Some(href) = link.value().attr("href") {
                    // Skip anchor links and other non-page links
                    if href.starts_with('#') || href.is_empty() {
                        continue;
                    }

                    let full_url = if href.starts_with('/') {
                        format!("https://docs.rs{href}")
                    } else if href.starts_with("http") {
                        href.to_string()
                    } else if href.starts_with("../") || href.starts_with("./") {
                        // Relative links
                        continue;
                    } else {
                        // Relative links without prefix - resolve relative to current URL
                        let current_base = if url.ends_with('/') {
                            url.clone()
                        } else {
                            // Remove filename and keep directory
                            let mut parts: Vec<&str> = url.split('/').collect();
                            if parts.last().is_some_and(|p| p.contains('.')) {
                                parts.pop(); // Remove filename
                            }
                            format!("{}/", parts.join("/"))
                        };
                        format!("{current_base}{href}")
                    };

                    // Only follow links within the same crate's documentation, and skip fragments
                    if full_url.contains(&format!("docs.rs/{crate_name}/"))
                        && !full_url.contains('#')
                        && !visited.contains(&full_url)
                        && to_visit.len() < max_pages * 2
                    {
                        // Prevent queue explosion
                        to_visit.push_back(full_url);
                    }
                }
            }
        }

        // Small delay to be respectful to docs.rs
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    println!(
        "✅ Scan complete: found {doc_pages_found} documentation pages in {processed} total pages"
    );
    Ok(doc_pages_found)
}

async fn fetch_with_retry(
    client: &reqwest::Client,
    url: &str,
    retries: usize,
) -> Result<String, ServerError> {
    for attempt in 0..retries {
        match client.get(url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    return response
                        .text()
                        .await
                        .map_err(|e| ServerError::Network(e.to_string()));
                } else if response.status().as_u16() == 429 {
                    // Rate limited, wait and retry
                    let wait_time = Duration::from_secs(2_u64.pow(attempt as u32));
                    tokio::time::sleep(wait_time).await;
                    continue;
                } else if response.status().is_client_error() {
                    // 4xx errors (including 404) are permanent failures - don't retry
                    return Err(ServerError::Network(format!(
                        "HTTP {}: {}",
                        response.status(),
                        url
                    )));
                } else {
                    // 5xx server errors should be retried
                    if attempt == retries - 1 {
                        return Err(ServerError::Network(format!(
                            "HTTP {}: {}",
                            response.status(),
                            url
                        )));
                    }
                    let wait_time = Duration::from_secs(2_u64.pow(attempt as u32));
                    tokio::time::sleep(wait_time).await;
                    continue;
                }
            }
            Err(e) => {
                if attempt == retries - 1 {
                    return Err(ServerError::Network(e.to_string()));
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    Err(ServerError::Network("Max retries exceeded".to_string()))
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    let cli = Cli::parse();

    // Check if crate exists on docs.rs first
    let test_url = format!(
        "https://docs.rs/{}/latest/{}/",
        cli.crate_name, cli.crate_name
    );
    let client = reqwest::Client::new();
    let response = client
        .head(&test_url)
        .send()
        .await
        .map_err(|e| ServerError::Network(e.to_string()))?;

    if !response.status().is_success() {
        return Err(ServerError::Config(format!(
            "Crate '{}' not found on docs.rs (HTTP {}). Please verify the crate name.",
            cli.crate_name,
            response.status()
        )));
    }

    // Scan for expected document count
    let expected_docs = scan_crate_docs_count(&cli.crate_name, cli.max_scan_pages).await?;

    // Load existing proxy config
    let config_path = "proxy-config.json";
    let mut config: ProxyConfig = if Path::new(config_path).exists() {
        let content = fs::read_to_string(config_path)
            .map_err(|e| ServerError::Config(format!("Failed to read {config_path}: {e}")))?;
        serde_json::from_str(&content)
            .map_err(|e| ServerError::Config(format!("Failed to parse {config_path}: {e}")))?
    } else {
        ProxyConfig {
            rustdocs_binary_path: "rustdocs_mcp_server".to_string(),
            crates: Vec::new(),
        }
    };

    // Check if crate already exists
    if let Some(existing) = config.crates.iter_mut().find(|c| c.name == cli.crate_name) {
        if !cli.force {
            return Err(ServerError::Config(format!(
                "Crate '{}' already exists in proxy-config.json. Use --force to update.",
                cli.crate_name
            )));
        }

        println!("📝 Updating existing crate '{}'", cli.crate_name);
        existing.features = cli.features;
        existing.enabled = cli.enabled;
        existing.expected_docs = Some(expected_docs);
    } else {
        println!("➕ Adding new crate '{}'", cli.crate_name);
        config.crates.push(CrateConfig {
            name: cli.crate_name.clone(),
            features: cli.features,
            enabled: cli.enabled,
            expected_docs: Some(expected_docs),
        });
    }

    // Sort crates alphabetically for consistency
    config.crates.sort_by(|a, b| a.name.cmp(&b.name));

    // Write updated config back to file
    let updated_content = serde_json::to_string_pretty(&config)
        .map_err(|e| ServerError::Config(format!("Failed to serialize config: {e}")))?;

    fs::write(config_path, updated_content)
        .map_err(|e| ServerError::Config(format!("Failed to write {config_path}: {e}")))?;

    println!(
        "✅ Successfully added/updated '{}' in proxy-config.json",
        cli.crate_name
    );
    println!("📊 Expected documents: {expected_docs}");

    // Optional: Show current database stats for this crate
    if let Ok(db) = Database::new().await {
        if let Ok(current_count) = db.count_crate_documents(&cli.crate_name).await {
            if current_count > 0 {
                println!("📚 Current documents in database: {current_count}");
                if current_count < expected_docs {
                    println!("⚠️  Database has fewer docs than expected ({current_count} < {expected_docs})");
                    println!("💡 Run the server to trigger automatic backfill, or use 'cargo run --bin populate_db -- --crate-name {}'", cli.crate_name);
                }
            } else {
                println!("📚 No documents in database yet for this crate");
                println!(
                    "💡 Run 'cargo run --bin populate_db -- --crate-name {}' to populate",
                    cli.crate_name
                );
            }
        }
    }

    Ok(())
}
