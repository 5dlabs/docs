-- Migration script to update from 1536 to 3072 dimensions

-- Step 1: Add a new column for 3072-dimensional embeddings
ALTER TABLE doc_embeddings 
ADD COLUMN embedding_3072 vector(3072);

-- Step 2: Drop the old index
DROP INDEX IF EXISTS idx_doc_embeddings_vector;

-- Step 3: We'll need to regenerate all embeddings, so let's clear the old ones
-- (They're incompatible anyway since they used different models)
UPDATE doc_embeddings SET embedding = NULL;

-- Step 4: Drop the old column and rename the new one
ALTER TABLE doc_embeddings DROP COLUMN embedding;
ALTER TABLE doc_embeddings RENAME COLUMN embedding_3072 TO embedding;

-- Step 5: Update the search function for 3072 dimensions
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
    AND de.embedding IS NOT NULL
    ORDER BY de.embedding <=> query_embedding
    LIMIT limit_results;
END;
$$ LANGUAGE plpgsql;

-- Step 6: Create new index for 3072 dimensions
CREATE INDEX idx_doc_embeddings_vector
ON doc_embeddings
USING ivfflat (embedding vector_cosine_ops)
WITH (lists = 100);

-- Show the updated schema
\d doc_embeddings

-- Show stats
SELECT 
    COUNT(*) as total_docs,
    COUNT(embedding) as docs_with_embeddings
FROM doc_embeddings;