#!/usr/bin/env fish
# Discord Bot Mock Test Runner
# Usage: fish tests/bot_mock/run_discord_test.fish

set SCRIPT_DIR (dirname (realpath (status filename)))
set PROJECT_ROOT (realpath $SCRIPT_DIR/../..)
set MOCK_PORT 5001
set VENV_PYTHON $SCRIPT_DIR/.venv/bin/python
set BOT_BIN $PROJECT_ROOT/target/debug/octos
set BOT_LOG /tmp/octos_discord_bot_test.log
set CONFIG_FILE $PROJECT_ROOT/.octos/test_discord_config.json

# ── Colors ───────────────────────────────────────────────────────────────────
set RED   '\033[0;31m'
set GREEN '\033[0;32m'
set YELLOW '\033[0;33m'
set CYAN  '\033[0;36m'
set GRAY  '\033[0;90m'
set BOLD  '\033[1m'
set RESET '\033[0m'

function info
    echo -e "$CYAN  ℹ $RESET $argv"
end
function ok
    echo -e "$GREEN  ✅ $RESET $argv"
end
function warn
    echo -e "$YELLOW  ⚠️  $RESET $argv"
end
function err
    echo -e "$RED  ❌ $RESET $argv"
end
function section
    echo ""
    echo -e "$BOLD$CYAN── $argv $RESET"
end
function log_line
    echo -e "$GRAY    $argv$RESET"
end

# ── 1. Check required env vars ───────────────────────────────────────────────
section "Checking environment"
if not set -q ANTHROPIC_API_KEY
    err "ANTHROPIC_API_KEY is not set"
    exit 1
end

# In mock mode, DISCORD_BOT_TOKEN is not a real Discord token — the mock server
# doesn't validate it. Auto-set a dummy value if not provided.
if not set -q DISCORD_BOT_TOKEN
    set -x DISCORD_BOT_TOKEN "mock-bot-token-for-testing"
    info "DISCORD_BOT_TOKEN not set, using dummy value (mock mode)"
end
ok "Environment variables present"

# ── 2. Check Python venv ─────────────────────────────────────────────────────
if not test -f $VENV_PYTHON
    info "Creating Python venv..."
    uv venv $SCRIPT_DIR/.venv
    uv pip install fastapi uvicorn httpx pytest pytest-asyncio websockets --python $VENV_PYTHON
end
ok "Python venv ready (with websockets for Discord WS support)"

# ── 3. Write test config (Discord channel) ───────────────────────────────────
section "Writing config"
mkdir -p (dirname $CONFIG_FILE)
echo '{
  "version": 1,
  "provider": "anthropic",
  "model": "MiniMax-M2.7",
  "api_key_env": "ANTHROPIC_API_KEY",
  "base_url": "https://api.minimaxi.com/anthropic",
  "gateway": {
    "channels": [
      {
        "type": "discord",
        "settings": {
          "token_env": "DISCORD_BOT_TOKEN"
        },
        "allowed_senders": []
      }
    ]
  }
}' > $CONFIG_FILE
ok "Config written to $CONFIG_FILE (discord channel)"

# ── 4. Kill anything on the mock port ────────────────────────────────────────
section "Preparing mock server"
set EXISTING_PID (lsof -ti tcp:$MOCK_PORT 2>/dev/null)
if test -n "$EXISTING_PID"
    warn "Port $MOCK_PORT in use by PID $EXISTING_PID, killing..."
    kill $EXISTING_PID 2>/dev/null
    for i in (seq 1 10)
        sleep 0.5
        if not lsof -ti tcp:$MOCK_PORT >/dev/null 2>&1
            break
        end
    end
end

# ── 5. Start Discord mock server (REST + WebSocket) ─────────────────────────
set -x PYTHONPATH $SCRIPT_DIR
$VENV_PYTHON -c "
import time, signal, sys
from mock_discord import MockDiscordServer
server = MockDiscordServer(port=$MOCK_PORT)
server.start_background()
print('ready', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
" &
set MOCK_PID $last_pid

# Wait for health check
sleep 2  # Give extra time for WS endpoint to be ready
if not $VENV_PYTHON -c "
import httpx, sys
try:
    r = httpx.get('http://127.0.0.1:$MOCK_PORT/health', timeout=5)
    sys.exit(0 if r.status_code == 200 else 1)
except Exception as e:
    print(e); sys.exit(1)
" 2>/dev/null
    err "Discord Mock server failed to start"
    lsof -i tcp:$MOCK_PORT
    exit 1
end
ok "Discord Mock server running on port $MOCK_PORT (PID $MOCK_PID)"

# ── 6. Build bot with discord feature ────────────────────────────────────────
section "Building octos (--features discord)"
info "This may take a moment on first build..."
set BUILD_LOG /tmp/octos_discord_build.log
cargo build --manifest-path $PROJECT_ROOT/Cargo.toml --bin octos --features discord > $BUILD_LOG 2>&1
if test $status -ne 0
    err "Build failed:"
    cat $BUILD_LOG
    kill $MOCK_PID 2>/dev/null
    exit 1
end
ok "Build complete (discord feature)"

# ── 7. Start bot ──────────────────────────────────────────────────────────────
section "Starting octos gateway"
rm -f $BOT_LOG
# DISCORD_API_BASE_URL tells serenity's HttpBuilder.proxy() to replace
# https://discord.com with our mock server URL. No forward proxy needed.
set -x DISCORD_API_BASE_URL "http://127.0.0.1:$MOCK_PORT"
$BOT_BIN gateway --config $CONFIG_FILE > $BOT_LOG 2>&1 &
set BOT_PID $last_pid
info "Bot PID: $BOT_PID  |  Log: $BOT_LOG  |  API: $DISCORD_API_BASE_URL"

echo ""
echo -e "$GRAY  Waiting for gateway to start...$RESET"
set READY 0
for i in (seq 1 50)
    sleep 1
    if test -f $BOT_LOG
        set LINES (cat $BOT_LOG | wc -l | string trim)
        if test $LINES -gt 0
            tail -1 $BOT_LOG | read LAST_LINE
            echo -e "$GRAY  › $LAST_LINE$RESET"
        end
    end
    # Look for ready signal or bot connected message
    if grep -q "gateway.*ready\|Gateway ready\|\[gateway\] ready" $BOT_LOG 2>/dev/null
        set READY 1
        break
    end
    if grep -q "Discord.*bot connected\|Discord channel started" $BOT_LOG 2>/dev/null
        set READY 1
        break
    end
    if grep -q "^Error:" $BOT_LOG 2>/dev/null
        break
    end
end

if test $READY -eq 0
    err "Bot failed to start. Full log:"
    echo ""
    cat $BOT_LOG | while read line
        log_line $line
    end
    echo ""
    kill $BOT_PID 2>/dev/null
    kill $MOCK_PID 2>/dev/null
    exit 1
end
ok "Gateway ready! (Discord channel active)"

# ── 8. Run tests ──────────────────────────────────────────────────────────────
section "Running tests"

set -x PYTHONPATH $SCRIPT_DIR
set -x MOCK_BASE_URL http://127.0.0.1:$MOCK_PORT

$VENV_PYTHON -m pytest $SCRIPT_DIR/test_discord.py -v --tb=short --no-header

set TEST_EXIT $status

# ── 9. Cleanup ────────────────────────────────────────────────────────────────
section "Cleanup"
kill $BOT_PID 2>/dev/null
kill $MOCK_PID 2>/dev/null
ok "Processes stopped"

echo ""
if test $TEST_EXIT -eq 0
    echo -e "$BOLD$GREEN  🎉 All Discord tests passed!$RESET"
else
    echo -e "$BOLD$RED  💥 Some Discord tests failed$RESET"
    echo -e "$GRAY  Bot log: $BOT_LOG$RESET"
end
echo ""
exit $TEST_EXIT
