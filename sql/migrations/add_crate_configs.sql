-- Migration: Add crate configuration tables
-- This replaces the proxy-config.json file with database-driven configuration

-- Table to store crate configurations
CREATE TABLE IF NOT EXISTS crate_configs (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    version_spec TEXT NOT NULL,     -- "latest" or specific version like "1.77.0"
    current_version TEXT,           -- The actual version currently stored
    features TEXT[],                -- Array of features like ['full', 'macros']
    expected_docs INTEGER NOT NULL DEFAULT 0,
    enabled BOOLEAN DEFAULT true,
    last_checked TIMESTAMPTZ,       -- When we last checked for updates
    last_populated TIMESTAMPTZ,     -- When we last populated docs
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(name, version_spec)
);

-- Create index for faster lookups
CREATE INDEX idx_crate_configs_name ON crate_configs(name);
CREATE INDEX idx_crate_configs_enabled ON crate_configs(enabled);

-- Table to track population jobs
CREATE TABLE IF NOT EXISTS population_jobs (
    id SERIAL PRIMARY KEY,
    crate_config_id INTEGER REFERENCES crate_configs(id),
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    error_message TEXT,
    docs_populated INTEGER,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Create index for job queries
CREATE INDEX idx_population_jobs_status ON population_jobs(status);
CREATE INDEX idx_population_jobs_crate_config_id ON population_jobs(crate_config_id);

-- Function to update the updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ language 'plpgsql';

-- Create trigger for crate_configs
CREATE TRIGGER update_crate_configs_updated_at BEFORE UPDATE
    ON crate_configs FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Migrate existing crates if any exist
INSERT INTO crate_configs (name, version_spec, current_version, features, expected_docs, enabled)
SELECT 
    c.name,
    'latest' as version_spec,
    c.version as current_version,
    ARRAY[]::TEXT[] as features,
    COUNT(de.id) as expected_docs,
    true as enabled
FROM crates c
LEFT JOIN doc_embeddings de ON c.id = de.crate_id
GROUP BY c.id, c.name, c.version
ON CONFLICT (name, version_spec) DO NOTHING;