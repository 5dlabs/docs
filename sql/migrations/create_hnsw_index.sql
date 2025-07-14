-- Create HNSW index for 3072 dimensions (better for high dimensions)

-- First check if we have the pgvector extension with HNSW support
SELECT version() as postgres_version, 
       extversion as pgvector_version 
FROM pg_extension 
WHERE extname = 'vector';

-- Create HNSW index (better performance for high dimensions)
-- HNSW doesn't have the 2000 dimension limit that IVFFlat has
CREATE INDEX idx_doc_embeddings_vector_hnsw
ON doc_embeddings
USING hnsw (embedding vector_cosine_ops);

-- If HNSW is not available, create a regular btree index for now
-- The vector search will still work, just slower without an optimized index

-- Show indexes
\di doc_embeddings*