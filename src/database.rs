use crate::error::ServerError;
use ndarray::Array1;
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::{env, time::Duration};

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

#[allow(dead_code)] // Some methods are only used by specific binaries
impl Database {
    pub async fn new() -> Result<Self, ServerError> {
        let database_url = env::var("MCPDOCS_DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://jonathonfritz@localhost/rust_docs_vectors".to_string()
        });

        let pool = PgPoolOptions::new()
            .max_connections(10) // Increased from 5
            .idle_timeout(Duration::from_secs(300)) // Close idle after 5min
            .max_lifetime(Duration::from_secs(1800)) // Refresh after 30min
            .acquire_timeout(Duration::from_secs(30)) // Timeout waiting for connection
            .connect(&database_url)
            .await
            .map_err(|e| ServerError::Database(format!("Failed to connect to database: {e}")))?;

        Ok(Self { pool })
    }

    /// Insert or update a crate in the database
    pub async fn upsert_crate(
        &self,
        crate_name: &str,
        version: Option<&str>,
    ) -> Result<i32, ServerError> {
        let result = sqlx::query(
            r#"
            INSERT INTO crates (name, version)
            VALUES ($1, $2)
            ON CONFLICT (name)
            DO UPDATE SET
                version = COALESCE($2, crates.version),
                last_updated = CURRENT_TIMESTAMP
            RETURNING id
            "#,
        )
        .bind(crate_name)
        .bind(version)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to upsert crate: {e}")))?;

        let id: i32 = result.get("id");
        Ok(id)
    }

    /// Check if embeddings exist for a crate
    pub async fn has_embeddings(&self, crate_name: &str) -> Result<bool, ServerError> {
        let result = sqlx::query(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM doc_embeddings WHERE crate_name = $1
            ) as exists
            "#,
        )
        .bind(crate_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to check embeddings: {e}")))?;

        let exists: bool = result.get("exists");
        Ok(exists)
    }

    /// Get all crates that have embeddings
    pub async fn get_all_crates_with_embeddings(&self) -> Result<Vec<String>, ServerError> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT crate_name FROM doc_embeddings
            ORDER BY crate_name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to get crates with embeddings: {e}")))?;

        let crates: Vec<String> = rows.iter().map(|row| row.get("crate_name")).collect();
        Ok(crates)
    }

    /// Insert a document embedding
    pub async fn insert_embedding(
        &self,
        crate_id: i32,
        crate_name: &str,
        doc_path: &str,
        content: &str,
        embedding: &Array1<f32>,
        token_count: i32,
    ) -> Result<(), ServerError> {
        let embedding_vec = Vector::from(embedding.to_vec());

        sqlx::query(
            r#"
            INSERT INTO doc_embeddings (crate_id, crate_name, doc_path, content, embedding, token_count)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (crate_name, doc_path)
            DO UPDATE SET
                content = $4,
                embedding = $5,
                token_count = $6,
                created_at = CURRENT_TIMESTAMP
            "#
        )
        .bind(crate_id)
        .bind(crate_name)
        .bind(doc_path)
        .bind(content)
        .bind(embedding_vec)
        .bind(token_count)
        .execute(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to insert embedding: {e}")))?;

        Ok(())
    }

    /// Batch insert multiple embeddings (more efficient)
    pub async fn insert_embeddings_batch(
        &self,
        crate_id: i32,
        crate_name: &str,
        embeddings: &[(String, String, Array1<f32>, i32)], // (path, content, embedding, token_count)
    ) -> Result<(), ServerError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ServerError::Database(format!("Failed to begin transaction: {e}")))?;

        for (doc_path, content, embedding, token_count) in embeddings {
            let embedding_vec = Vector::from(embedding.to_vec());

            sqlx::query(
                r#"
                INSERT INTO doc_embeddings (crate_id, crate_name, doc_path, content, embedding, token_count)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (crate_name, doc_path)
                DO UPDATE SET
                    content = $4,
                    embedding = $5,
                    token_count = $6,
                    created_at = CURRENT_TIMESTAMP
                "#
            )
            .bind(crate_id)
            .bind(crate_name)
            .bind(doc_path)
            .bind(content)
            .bind(embedding_vec)
            .bind(*token_count)
            .execute(&mut *tx)
            .await
            .map_err(|e| ServerError::Database(format!("Failed to insert embedding: {e}")))?;
        }

        tx.commit()
            .await
            .map_err(|e| ServerError::Database(format!("Failed to commit transaction: {e}")))?;

        // Update crate statistics
        self.update_crate_stats(crate_id).await?;

        Ok(())
    }

    /// Update crate statistics
    async fn update_crate_stats(&self, crate_id: i32) -> Result<(), ServerError> {
        sqlx::query(
            r#"
            UPDATE crates
            SET total_docs = (
                SELECT COUNT(*) FROM doc_embeddings WHERE crate_id = $1
            ),
            total_tokens = (
                SELECT COALESCE(SUM(token_count), 0) FROM doc_embeddings WHERE crate_id = $1
            )
            WHERE id = $1
            "#,
        )
        .bind(crate_id)
        .execute(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to update crate stats: {e}")))?;

        Ok(())
    }

    /// Search for similar documents using vector similarity
    pub async fn search_similar_docs(
        &self,
        crate_name: &str,
        query_embedding: &Array1<f32>,
        limit: i32,
    ) -> Result<Vec<(String, String, f32)>, ServerError> {
        let embedding_vec = Vector::from(query_embedding.to_vec());

        let results = sqlx::query(
            r#"
            SELECT
                doc_path,
                content,
                1 - (embedding <=> $1) as similarity
            FROM doc_embeddings
            WHERE crate_name = $2
            ORDER BY embedding <=> $1
            LIMIT $3
            "#,
        )
        .bind(embedding_vec)
        .bind(crate_name)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to search documents: {e}")))?;

        Ok(results
            .into_iter()
            .map(|row| {
                let doc_path: String = row.get("doc_path");
                let content: String = row.get("content");
                let similarity: f64 = row.get("similarity");
                #[allow(clippy::cast_possible_truncation)]
                let similarity = similarity as f32; // Convert to f32 for compatibility
                (doc_path, content, similarity)
            })
            .collect())
    }

    /// Get all documents for a crate (for loading into memory if needed)
    pub async fn get_crate_documents(
        &self,
        crate_name: &str,
    ) -> Result<Vec<(String, String, Array1<f32>)>, ServerError> {
        eprintln!("    🔍 Querying database for crate: {crate_name}");
        let query_start = std::time::Instant::now();

        let results = sqlx::query(
            r#"
            SELECT doc_path, content, embedding
            FROM doc_embeddings
            WHERE crate_name = $1
            ORDER BY doc_path
            "#,
        )
        .bind(crate_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to get crate documents: {e}")))?;

        let query_time = query_start.elapsed();
        eprintln!(
            "    📊 Found {} documents for {} in {:.3}s",
            results.len(),
            crate_name,
            query_time.as_secs_f64()
        );

        let mut documents = Vec::new();
        for (i, row) in results.iter().enumerate() {
            let doc_path: String = row.get("doc_path");
            let content: String = row.get("content");
            let embedding_vec: Vector = row.get("embedding");
            let embedding_array = Array1::from_vec(embedding_vec.to_vec());

            if i < 3 || (i + 1) % 5 == 0 {
                eprintln!(
                    "    📄 [{}/{}] Processed: {} ({} chars, {} dims)",
                    i + 1,
                    results.len(),
                    doc_path,
                    content.len(),
                    embedding_array.len()
                );
            }

            documents.push((doc_path, content, embedding_array));
        }

        Ok(documents)
    }

    /// Delete all embeddings for a crate
    pub async fn delete_crate_embeddings(&self, crate_name: &str) -> Result<(), ServerError> {
        sqlx::query(
            r#"
            DELETE FROM doc_embeddings WHERE crate_name = $1
            "#,
        )
        .bind(crate_name)
        .execute(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to delete embeddings: {e}")))?;

        Ok(())
    }

    /// Get crate statistics
    pub async fn get_crate_stats(&self) -> Result<Vec<CrateStats>, ServerError> {
        let results = sqlx::query(
            r#"
            SELECT
                name,
                version,
                last_updated,
                total_docs,
                total_tokens
            FROM crates
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to get crate stats: {e}")))?;

        Ok(results
            .into_iter()
            .map(|row| {
                let name: String = row.get("name");
                let version: Option<String> = row.get("version");
                let last_updated: chrono::NaiveDateTime = row.get("last_updated");
                let total_docs: Option<i32> = row.get("total_docs");
                let total_tokens: Option<i32> = row.get("total_tokens");

                CrateStats {
                    name,
                    version,
                    last_updated,
                    total_docs: total_docs.unwrap_or(0),
                    total_tokens: total_tokens.unwrap_or(0),
                }
            })
            .collect())
    }

    /// Count documents for a specific crate
    pub async fn count_crate_documents(&self, crate_name: &str) -> Result<usize, ServerError> {
        let result = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM doc_embeddings
            WHERE crate_name = $1
            "#,
        )
        .bind(crate_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to count crate documents: {e}")))?;

        let count: i64 = result.get("count");
        Ok(count as usize)
    }

    // ===== Crate Configuration Methods =====

    /// Get all crate configurations
    pub async fn get_crate_configs(
        &self,
        enabled_only: bool,
    ) -> Result<Vec<CrateConfig>, ServerError> {
        let query = if enabled_only {
            "SELECT * FROM crate_configs WHERE enabled = true ORDER BY name, version_spec"
        } else {
            "SELECT * FROM crate_configs ORDER BY name, version_spec"
        };

        let configs = sqlx::query_as::<_, CrateConfig>(query)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ServerError::Database(format!("Failed to get crate configs: {e}")))?;

        Ok(configs)
    }

    /// Get a specific crate configuration
    pub async fn get_crate_config(
        &self,
        name: &str,
        version_spec: &str,
    ) -> Result<Option<CrateConfig>, ServerError> {
        let config = sqlx::query_as::<_, CrateConfig>(
            "SELECT * FROM crate_configs WHERE name = $1 AND version_spec = $2",
        )
        .bind(name)
        .bind(version_spec)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to get crate config: {e}")))?;

        Ok(config)
    }

    /// Add or update a crate configuration
    pub async fn upsert_crate_config(
        &self,
        config: &CrateConfig,
    ) -> Result<CrateConfig, ServerError> {
        let result = sqlx::query_as::<_, CrateConfig>(
            r#"
            INSERT INTO crate_configs (name, version_spec, current_version, features, expected_docs, enabled)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (name, version_spec) DO UPDATE SET
                current_version = EXCLUDED.current_version,
                features = EXCLUDED.features,
                expected_docs = EXCLUDED.expected_docs,
                enabled = EXCLUDED.enabled,
                updated_at = CURRENT_TIMESTAMP
            RETURNING *
            "#
        )
        .bind(&config.name)
        .bind(&config.version_spec)
        .bind(&config.current_version)
        .bind(&config.features)
        .bind(config.expected_docs)
        .bind(config.enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to upsert crate config: {e}")))?;

        Ok(result)
    }

    /// Delete a crate configuration
    pub async fn delete_crate_config(
        &self,
        name: &str,
        version_spec: &str,
    ) -> Result<bool, ServerError> {
        let result = sqlx::query("DELETE FROM crate_configs WHERE name = $1 AND version_spec = $2")
            .bind(name)
            .bind(version_spec)
            .execute(&self.pool)
            .await
            .map_err(|e| ServerError::Database(format!("Failed to delete crate config: {e}")))?;

        Ok(result.rows_affected() > 0)
    }

    /// Check which crates need population or updates
    pub async fn get_crates_needing_update(&self) -> Result<Vec<CrateConfig>, ServerError> {
        let configs = sqlx::query_as::<_, CrateConfig>(
            r#"
            SELECT cc.* FROM crate_configs cc
            LEFT JOIN crates c ON cc.name = c.name AND cc.current_version = c.version
            WHERE cc.enabled = true
            AND (
                c.id IS NULL  -- Crate doesn't exist
                OR cc.last_populated IS NULL  -- Never populated
                OR (cc.version_spec = 'latest' AND cc.last_checked < CURRENT_TIMESTAMP - INTERVAL '24 hours')  -- Check for updates daily
            )
            ORDER BY cc.name
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to get crates needing update: {e}")))?;

        Ok(configs)
    }

    /// Create a population job
    pub async fn create_population_job(&self, crate_config_id: i32) -> Result<i32, ServerError> {
        let result = sqlx::query(
            r#"
            INSERT INTO population_jobs (crate_config_id, status, created_at)
            VALUES ($1, 'pending', CURRENT_TIMESTAMP)
            RETURNING id
            "#,
        )
        .bind(crate_config_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ServerError::Database(format!("Failed to create population job: {e}")))?;

        Ok(result.get("id"))
    }

    /// Update population job status
    pub async fn update_population_job(
        &self,
        job_id: i32,
        status: &str,
        error_message: Option<&str>,
        docs_populated: Option<i32>,
    ) -> Result<(), ServerError> {
        let mut query = "UPDATE population_jobs SET status = $1".to_string();
        let mut param_count = 1;

        if status == "running" {
            query.push_str(", started_at = CURRENT_TIMESTAMP");
        } else if status == "completed" || status == "failed" {
            query.push_str(", completed_at = CURRENT_TIMESTAMP");
        }

        if let Some(_error) = error_message {
            param_count += 1;
            query.push_str(&format!(", error_message = ${param_count}"));
        }

        if let Some(_docs) = docs_populated {
            param_count += 1;
            query.push_str(&format!(", docs_populated = ${param_count}"));
        }

        query.push_str(&format!(" WHERE id = ${}", param_count + 1));

        let mut q = sqlx::query(&query).bind(status);

        if let Some(error) = error_message {
            q = q.bind(error);
        }

        if let Some(docs) = docs_populated {
            q = q.bind(docs);
        }

        q.bind(job_id)
            .execute(&self.pool)
            .await
            .map_err(|e| ServerError::Database(format!("Failed to update population job: {e}")))?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct CrateStats {
    pub name: String,
    pub version: Option<String>,
    pub last_updated: chrono::NaiveDateTime,
    pub total_docs: i32,
    pub total_tokens: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CrateConfig {
    pub id: i32,
    pub name: String,
    pub version_spec: String,
    pub current_version: Option<String>,
    pub features: Vec<String>,
    pub expected_docs: i32,
    pub enabled: bool,
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
    pub last_populated: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
