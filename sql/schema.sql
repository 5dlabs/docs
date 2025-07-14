-- Schema for Rust documentation vector database

-- Table to store crate information
CREATE TABLE IF NOT EXISTS crates (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) UNIQUE NOT NULL,
    version VARCHAR(50),
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    total_docs INTEGER DEFAULT 0,
    total_tokens INTEGER DEFAULT 0
);

-- Table to store document embeddings
CREATE TABLE IF NOT EXISTS doc_embeddings (
    id SERIAL PRIMARY KEY,
    crate_id INTEGER REFERENCES crates(id) ON DELETE CASCADE,
    crate_name VARCHAR(255) NOT NULL, -- Denormalized for faster queries
    doc_path TEXT NOT NULL,
    content TEXT NOT NULL,
    embedding vector(3072), -- OpenAI text-embedding-3-large dimension
    token_count INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(crate_name, doc_path)
);

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_doc_embeddings_crate_name ON doc_embeddings(crate_name);
CREATE INDEX IF NOT EXISTS idx_doc_embeddings_crate_id ON doc_embeddings(crate_id);

-- Note: pgvector indexes (IVFFlat and HNSW) have a 2000 dimension limit
-- For 3072 dimensions, we skip the index. Queries will still work but be slower.
-- Consider upgrading pgvector or using 1536 dimensions if performance is critical.

-- Function to search for similar documents
CREATE OR REPLACE FUNCTION search_similar_docs(
    query_embedding vector(3072),
    target_crate_name VARCHAR(255),
    limit_results INTEGER DEFAULT 5
)
RETURNS TABLE (
    id INTEGER,
    crate_name VARCHAR(255),
    doc_path TEXT,
    content TEXT,
    similarity FLOAT
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        de.id,
        de.crate_name,
        de.doc_path,
        de.content,
        1 - (de.embedding <=> query_embedding) AS similarity
    FROM doc_embeddings de
    WHERE de.crate_name = target_crate_name
    ORDER BY de.embedding <=> query_embedding
    LIMIT limit_results;
END;
$$ LANGUAGE plpgsql;

-- View for crate statistics
CREATE OR REPLACE VIEW crate_stats AS
SELECT
    c.name,
    c.version,
    c.last_updated,
    COUNT(de.id) as doc_count,
    COALESCE(SUM(de.token_count), 0) as total_tokens,
    pg_size_pretty(pg_total_relation_size('doc_embeddings')) as table_size
FROM crates c
LEFT JOIN doc_embeddings de ON c.id = de.crate_id
GROUP BY c.id, c.name, c.version, c.last_updated;