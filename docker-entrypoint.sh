#!/bin/bash
set -e

# Check required environment variables
if [ -z "$MCPDOCS_DATABASE_URL" ]; then
    echo "Error: MCPDOCS_DATABASE_URL environment variable is required" >&2
    exit 1
fi

if [ -z "$OPENAI_API_KEY" ] && [ -z "$VOYAGE_API_KEY" ]; then
    echo "Error: Either OPENAI_API_KEY or VOYAGE_API_KEY environment variable is required" >&2
    exit 1
fi

# Execute the command
exec "$@"