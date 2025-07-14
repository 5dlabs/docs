#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
HTTP_PORT=3000

show_usage() {
    echo "Usage: $0 {start-http|start-stdio|stop|status|restart-http}"
    echo ""
    echo "Commands:"
    echo "  start-http    Start HTTP/SSE server on port $HTTP_PORT"
    echo "  start-stdio   Start stdio server for local use"
    echo "  stop          Stop all MCP servers"
    echo "  status        Show running servers"
    echo "  restart-http  Stop all and start HTTP server"
    exit 1
}

stop_all() {
    echo "🛑 Stopping all MCP servers..."
    pkill -f "rustdocs_mcp_server" || true
    pkill -f "rustdocs_mcp_server_http" || true
    sleep 1
}

show_status() {
    echo "📊 Current MCP server status:"
    echo ""
    
    # Check for running processes
    if pgrep -f "rustdocs_mcp_server" > /dev/null; then
        echo "🟢 Running servers:"
        ps aux | grep -E "(rustdocs_mcp_server|http_server)" | grep -v grep | while read -r line; do
            echo "   $line"
        done
    else
        echo "🔴 No MCP servers running"
    fi
    
    echo ""
    
    # Check port usage
    if lsof -i :$HTTP_PORT > /dev/null 2>&1; then
        echo "🌐 Port $HTTP_PORT status:"
        lsof -i :$HTTP_PORT
    else
        echo "🔌 Port $HTTP_PORT is available"
    fi
    
    echo ""
    
    # Check Claude Code MCP configuration
    echo "⚙️  Claude Code MCP configuration:"
    claude mcp list 2>/dev/null || echo "   Could not retrieve MCP configuration"
}

start_http() {
    echo "🚀 Starting HTTP/SSE server..."
    
    # Stop any existing servers
    stop_all
    
    # Load environment variables
    if [ -f "$PROJECT_DIR/.env" ]; then
        echo "📁 Loading environment from .env"
        export $(grep -v '^#' "$PROJECT_DIR/.env" | xargs)
    fi
    
    # Check required environment variables
    if [ -z "$MCPDOCS_DATABASE_URL" ]; then
        echo "❌ Error: MCPDOCS_DATABASE_URL not set"
        exit 1
    fi
    
    if [ -z "$OPENAI_API_KEY" ]; then
        echo "❌ Error: OPENAI_API_KEY not set"
        exit 1
    fi
    
    # Start HTTP server
    echo "🌐 Starting HTTP server on port $HTTP_PORT"
    cd "$PROJECT_DIR"
    nohup cargo run --release --bin rustdocs_mcp_server_http -- --all --port $HTTP_PORT > logs/http-server.log 2>&1 &
    
    # Wait for server to start
    echo "⏳ Waiting for server to start..."
    sleep 5
    
    # Verify server is running
    if curl -s http://localhost:$HTTP_PORT/sse --max-time 2 > /dev/null; then
        echo "✅ HTTP server started successfully"
        echo "📡 SSE endpoint: http://localhost:$HTTP_PORT/sse"
        echo "📤 POST endpoint: http://localhost:$HTTP_PORT/message"
    else
        echo "❌ Failed to start HTTP server"
        exit 1
    fi
}

start_stdio() {
    echo "🚀 Starting stdio server..."
    
    # Stop any existing servers
    stop_all
    
    echo "📝 Stdio server ready for Claude Code stdio transport"
    echo "   Configure with: claude mcp add rust-docs '$PROJECT_DIR/run_mcp_server.sh'"
}

# Create logs directory if it doesn't exist
mkdir -p "$PROJECT_DIR/logs"

case "$1" in
    start-http)
        start_http
        ;;
    start-stdio)
        start_stdio
        ;;
    stop)
        stop_all
        echo "✅ All servers stopped"
        ;;
    status)
        show_status
        ;;
    restart-http)
        start_http
        ;;
    *)
        show_usage
        ;;
esac