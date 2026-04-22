#!/usr/bin/env bash
# Quick test to verify Discord Mock Server Gateway fix

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV_PYTHON="$SCRIPT_DIR/.venv/bin/python"

echo "🧪 Testing Discord Mock Server Gateway Fix"
echo "=" | awk '{for(i=1;i<=70;i++) printf "="; print ""}'

# Clear cache first
echo -e "\n1️⃣  Clearing Python cache..."
find "$SCRIPT_DIR" -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
find "$SCRIPT_DIR" -name "*.pyc" -delete 2>/dev/null || true
echo "   ✅ Cache cleared"

# Start Mock Server
echo -e "\n2️⃣  Starting Mock Discord Server..."
PYTHONPATH="$SCRIPT_DIR" PYTHONUNBUFFERED=1 "$VENV_PYTHON" -c "
import time, signal, sys
from mock_discord import MockDiscordServer
server = MockDiscordServer(port=5001)
server.start_background()
print('Mock server started on port 5001', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
" &
MOCK_PID=$!

sleep 2

# Check health
echo -e "\n3️⃣  Checking health endpoint..."
if curl -s http://127.0.0.1:5001/health | grep -q "ok"; then
    echo "   ✅ Mock server is healthy"
else
    echo "   ❌ Mock server health check failed"
    kill $MOCK_PID 2>/dev/null || true
    exit 1
fi

# Run Gateway event test
echo -e "\n4️⃣  Running Gateway event dispatch test..."
"$VENV_PYTHON" "$SCRIPT_DIR/test_gateway_events.py"
TEST_RESULT=$?

# Cleanup
echo -e "\n5️⃣  Cleaning up..."
kill $MOCK_PID 2>/dev/null || true
wait $MOCK_PID 2>/dev/null || true
echo "   ✅ Mock server stopped"

if [[ $TEST_RESULT -eq 0 ]]; then
    echo -e "\n🎉 All tests passed! Gateway fix is working."
    exit 0
else
    echo -e "\n💥 Tests failed. Check the output above for details."
    exit 1
fi
