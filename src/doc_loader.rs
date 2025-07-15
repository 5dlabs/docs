use scraper::{Html, Selector};
use std::collections::{HashSet, VecDeque};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)] // Some variants are only used in specific contexts
pub enum DocLoaderError {
    #[error("HTTP Error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("CSS selector error: {0}")]
    Selector(String),
    #[error("Parsing error: {0}")]
    Parsing(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Rate limited: {0}")]
    RateLimited(String),
}

// Simple struct to hold document content
#[derive(Debug, Clone)]
pub struct Document {
    pub path: String,
    pub content: String,
}

// Result struct that includes version information
#[derive(Debug)]
#[allow(dead_code)] // Used by binaries
pub struct LoadResult {
    pub documents: Vec<Document>,
    pub version: Option<String>,
}

/// Load documentation from docs.rs for a given crate
#[allow(dead_code)] // Used by binaries
pub async fn load_documents_from_docs_rs(
    crate_name: &str,
    _version: &str,
    _features: Option<&Vec<String>>,
    max_pages: Option<usize>,
) -> Result<LoadResult, DocLoaderError> {
    println!("Fetching documentation from docs.rs for crate: {crate_name}");

    let base_url = format!("https://docs.rs/{crate_name}/latest/{crate_name}/");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| DocLoaderError::Network(e.to_string()))?;

    let mut documents = Vec::new();
    let mut visited = HashSet::new();
    let mut to_visit = VecDeque::new();
    to_visit.push_back(base_url.clone());
    let mut extracted_version = None;

    // Define the CSS selector for the main content area
    let content_selector = Selector::parse("div.docblock, section.docblock, .rustdoc .docblock")
        .map_err(|e| DocLoaderError::Selector(e.to_string()))?;

    let max_pages = max_pages.unwrap_or(10000); // Default to 10000 pages if not specified
    let mut processed = 0;

    // Helper function to check if a URL should be processed (filter out source code and other non-docs)
    fn should_process_url(url: &str) -> bool {
        // Skip source code pages
        if url.contains("/src/") {
            return false;
        }

        // Skip specific non-documentation patterns
        if url.contains("#method.")
            || url.contains("#impl-")
            || url.contains("#associatedtype.")
            || url.contains("#associatedconstant.")
        {
            return false;
        }

        // Only process actual documentation pages
        true
    }

    while let Some(url) = to_visit.pop_front() {
        if processed >= max_pages {
            eprintln!("Reached maximum page limit ({max_pages}), stopping");
            break;
        }

        if visited.contains(&url) {
            continue;
        }

        // Skip non-documentation URLs
        if !should_process_url(&url) {
            visited.insert(url.clone());
            continue;
        }

        visited.insert(url.clone());
        processed += 1;

        eprintln!("Processing page {processed}/{max_pages}: {url}");

        // Fetch the page with retry logic
        let html_content = match fetch_with_retry(&client, &url, 3).await {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Failed to fetch {url} after retries: {e}");
                continue;
            }
        };

        let document = Html::parse_document(&html_content);

        // Extract version from the first page (usually in the header)
        if extracted_version.is_none() && processed == 1 {
            // Try to find version in the docs.rs header
            // docs.rs shows version in format "crate-name 1.2.3"
            if let Ok(version_selector) = Selector::parse(".version") {
                if let Some(version_elem) = document.select(&version_selector).next() {
                    let version_text = version_elem.text().collect::<String>();
                    extracted_version = Some(version_text.trim().to_string());
                    eprintln!("Extracted version: {extracted_version:?}");
                }
            }

            // Alternative: Look in the title or URL path
            if extracted_version.is_none() {
                // The URL might contain version like /crate-name/1.2.3/
                if let Some(version_match) = url.split('/').nth_back(2) {
                    if version_match != "latest" && version_match.chars().any(|c| c.is_numeric()) {
                        extracted_version = Some(version_match.to_string());
                        eprintln!("Extracted version from URL: {extracted_version:?}");
                    }
                }
            }
        }

        // Extract text content from documentation blocks
        let mut page_content = Vec::new();
        for element in document.select(&content_selector) {
            let text_content: String = element
                .text()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<&str>>()
                .join("\n");

            if !text_content.is_empty() {
                page_content.push(text_content);
            }
        }

        if !page_content.is_empty() {
            let relative_path = url
                .strip_prefix("https://docs.rs/")
                .unwrap_or(&url)
                .to_string();

            let blocks = page_content.len();
            let chars = page_content.join("\n\n").len();
            eprintln!(
                "  -> Extracted content from: {relative_path} ({blocks} blocks, {chars} chars)"
            );

            documents.push(Document {
                path: relative_path,
                content: page_content.join("\n\n"),
            });
        } else {
            eprintln!("  -> No content extracted from: {url}");
        }

        // Extract links to other documentation pages within the same crate
        // Follow links for first 75% of pages to get deeper coverage
        if processed < (max_pages * 3 / 4) {
            let link_selector = Selector::parse("a").unwrap();
            let mut found_links = 0;
            let mut added_links = 0;

            for link in document.select(&link_selector) {
                if let Some(href) = link.value().attr("href") {
                    found_links += 1;

                    // Follow various types of relative links
                    let should_follow = href.starts_with("./") ||
                                       href.starts_with("../") ||
                                       // Add support for simple relative paths
                                       (!href.starts_with("http") &&
                                        !href.starts_with("#") &&
                                        !href.starts_with("/") &&
                                        href.ends_with(".html"));

                    if should_follow {
                        if let Ok(absolute_url) = reqwest::Url::parse(&url) {
                            if let Ok(new_url) = absolute_url.join(href) {
                                let new_url_str = new_url.to_string();
                                if new_url_str.contains("docs.rs")
                                    && new_url_str.contains(crate_name)
                                    && !visited.contains(&new_url_str)
                                    && should_process_url(&new_url_str)
                                {
                                    to_visit.push_back(new_url_str.clone());
                                    added_links += 1;
                                    if added_links <= 5 {
                                        // Only show first 5 for brevity
                                        eprintln!("  -> Adding link: {href}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            eprintln!("  Found {found_links} links, added {added_links} new ones to visit");
        }

        // Add a longer delay to be respectful to docs.rs and avoid rate limiting
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    let doc_count = documents.len();
    eprintln!("Finished loading {doc_count} documents from docs.rs");
    Ok(LoadResult {
        documents,
        version: extracted_version,
    })
}

/// Synchronous wrapper that uses current tokio runtime
#[allow(dead_code)] // Available for future use
pub fn load_documents(
    crate_name: &str,
    crate_version_req: &str,
    features: Option<&Vec<String>>,
) -> Result<LoadResult, DocLoaderError> {
    // Check if we're already in a tokio runtime
    if tokio::runtime::Handle::try_current().is_ok() {
        // We're in a runtime, but we can't use block_on.
        // We need to make this function async or use a different approach.
        // For now, let's return an error suggesting the async version
        return Err(DocLoaderError::Parsing(
            "Cannot run synchronous load_documents from within async context. Use load_documents_from_docs_rs directly.".to_string()
        ));
    }

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| DocLoaderError::Parsing(format!("Failed to create tokio runtime: {e}")))?;

    rt.block_on(load_documents_from_docs_rs(
        crate_name,
        crate_version_req,
        features,
        None,
    ))
}

/// Fetch a URL with retry logic and rate limiting
#[allow(dead_code)] // Used internally
async fn fetch_with_retry(
    client: &reqwest::Client,
    url: &str,
    max_retries: usize,
) -> Result<String, DocLoaderError> {
    let mut attempts = 0;
    let mut delay = Duration::from_millis(1000); // Start with 1 second

    loop {
        match client.get(url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.text().await {
                        Ok(text) => return Ok(text),
                        Err(e) => {
                            eprintln!("Failed to read response body for {url}: {e}");
                            if attempts >= max_retries {
                                return Err(DocLoaderError::Http(e));
                            }
                        }
                    }
                } else if response.status() == 429 {
                    // Rate limited
                    let retry_num = attempts + 1;
                    let max_retries_plus = max_retries + 1;
                    eprintln!("Rate limited for {url}, waiting {delay:?} before retry {retry_num}/{max_retries_plus}");
                    if attempts >= max_retries {
                        return Err(DocLoaderError::RateLimited(format!(
                            "Rate limited after {} attempts",
                            attempts + 1
                        )));
                    }
                } else if response.status() == 404 {
                    // 404 is a permanent failure - don't retry
                    eprintln!("⚠️  Page not found (404): {url} - skipping");
                    return Err(DocLoaderError::Network(format!(
                        "HTTP {}",
                        response.status()
                    )));
                } else if response.status().is_client_error() {
                    // Other 4xx errors are also permanent failures - don't retry
                    eprintln!("⚠️  Client error ({}): {url} - skipping", response.status());
                    return Err(DocLoaderError::Network(format!(
                        "HTTP {}",
                        response.status()
                    )));
                } else {
                    // 5xx server errors should be retried
                    eprintln!("HTTP error for {}: {}", url, response.status());
                    if attempts >= max_retries {
                        return Err(DocLoaderError::Network(format!(
                            "HTTP {}",
                            response.status()
                        )));
                    }
                }
            }
            Err(e) => {
                eprintln!("Network error for {url}: {e}");
                if attempts >= max_retries {
                    return Err(DocLoaderError::Http(e));
                }
            }
        }

        // Wait before retrying with exponential backoff
        tokio::time::sleep(delay).await;
        delay = std::cmp::min(delay * 2, Duration::from_secs(30)); // Cap at 30 seconds
        attempts += 1;
    }
}
