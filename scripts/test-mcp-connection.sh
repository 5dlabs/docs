#!/bin/bash

# Test script for MCP connection debugging
# Usage: ./test-mcp-connection.sh <server-url>

SERVER_URL=${1:-"http://localhost:3000"}
SSE_URL="${SERVER_URL}/sse"
POST_URL="${SERVER_URL}/message"

echo "🔍 Testing MCP Server Connection"
echo "Server: $SERVER_URL"
echo "SSE Endpoint: $SSE_URL"
echo "POST Endpoint: $POST_URL"
echo ""

# Test 1: Health Check
echo "1️⃣ Testing health endpoint..."
curl -s "${SERVER_URL}/health/ready" | jq . 2>/dev/null || echo "❌ Health check failed or not JSON"
echo ""

# Test 2: SSE Connection (background process)
echo "2️⃣ Testing SSE connection (10 seconds)..."
timeout 10 curl -N -H "Accept: text/event-stream" "$SSE_URL" &
SSE_PID=$!
sleep 2
if kill -0 $SSE_PID 2>/dev/null; then
    echo "✅ SSE connection established"
    wait $SSE_PID
else
    echo "❌ SSE connection failed"
fi
echo ""

# Test 3: POST Endpoint
echo "3️⃣ Testing POST endpoint..."
RESPONSE=$(curl -s -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' \
  "$POST_URL")

if echo "$RESPONSE" | jq . >/dev/null 2>&1; then
    echo "✅ POST endpoint responded with JSON:"
    echo "$RESPONSE" | jq .
else
    echo "❌ POST endpoint failed or returned non-JSON:"
    echo "$RESPONSE"
fi
echo ""

echo "🏁 Test complete"