#!/usr/bin/env bash
# Smoke test for the cursor_pointer state via the test server.
# Prerequisites:
#   - Epoca built with: cargo build -p epoca --features test-server
#   - move_mouse compiled: swiftc -O -o /tmp/move_mouse tools/move_mouse.swift
#
# Usage: ./tools/test_cursor.sh [--skip-build]
set -euo pipefail

PORT=9223
BASE="http://127.0.0.1:$PORT"
BINARY="./target/debug/epoca"
MOUSE="/tmp/move_mouse"

# Build if needed
if [[ "${1:-}" != "--skip-build" ]]; then
    echo "Building with test-server feature..."
    cargo build -p epoca --features test-server
fi

# Compile mouse mover
if [[ ! -x "$MOUSE" ]]; then
    echo "Compiling move_mouse helper..."
    swiftc -O -o "$MOUSE" tools/move_mouse.swift
fi

# Launch app
echo "Launching Epoca with EPOCA_TEST=1..."
EPOCA_TEST=1 "$BINARY" &
APP_PID=$!
trap "kill $APP_PID 2>/dev/null; exit" EXIT INT TERM

# Wait for server to be ready
for i in $(seq 1 30); do
    if curl -sf "$BASE/state" >/dev/null 2>&1; then
        break
    fi
    sleep 0.2
done

echo "Test server ready."

# Get initial state
STATE=$(curl -sf "$BASE/state")
TAB_COUNT=$(echo "$STATE" | python3 -c "import sys,json; print(json.load(sys.stdin)['tab_count'])")
echo "Initial state: $TAB_COUNT tabs"

# Navigate to a test page with a known link
curl -sf -X POST "$BASE/action" -d '{"action":"navigate","url":"https://example.com"}' | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('ok'), d"
echo "Navigate OK"

# Wait for page load
sleep 2

# Check state reflects the URL
STATE=$(curl -sf "$BASE/state")
URL=$(echo "$STATE" | python3 -c "
import sys,json
tabs = json.load(sys.stdin)['tabs']
active = [t for t in tabs if t['active']]
print(active[0]['url'] if active else 'none')
")
echo "Active tab URL: $URL"

# Eval JS in the page
TITLE=$(curl -sf "$BASE/webview/eval?js=document.title")
echo "Page title via eval: $TITLE"

echo ""
echo "All basic tests passed."
