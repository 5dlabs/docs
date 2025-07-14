-- Update schema to support text-embedding-3-large (3072 dimensions)

-- First, we need to drop the existing indexes that depend on the vector column
DROP INDEX IF EXISTS idx_doc_embeddings_vector;

-- Update the embedding column to support 3072 dimensions
ALTER TABLE doc_embeddings 
ALTER COLUMN embedding TYPE vector(3072);

-- Update the search function to use 3072 dimensions
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

-- Recreate the index with the new dimensions
CREATE INDEX idx_doc_embeddings_vector
ON doc_embeddings
USING ivfflat (embedding vector_cosine_ops)
WITH (lists = 100);

-- Show the changes
\d doc_embeddings