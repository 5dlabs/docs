#![allow(clippy::uninlined_format_args)] // Allow format! style for consistency

// Expose modules for use by binaries
pub mod database;
pub mod doc_loader;
pub mod embeddings;
pub mod error;
pub mod server;
