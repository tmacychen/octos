#!/usr/bin/env fish
# Telegram Bot Mock Test Runner
# Usage: fish tests/telegram_mock/run_test.fish

set SCRIPT_DIR (dirname (realpath (status filename)))
set PROJECT_ROOT (realpath $SCRIPT_DIR/../..)
set MOCK_PORT 5000
set VENV_PYTHON $SCRIPT_DIR/.venv/bin/python
set BOT_BIN $PROJECT_ROOT/target/debug/octos
set BOT_LOG /tmp/octos_bot_test.log
set CONFIG_FILE $PROJECT_ROOT/.octos/test_config.json

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
if not set -q TELEGRAM_BOT_TOKEN
    err "TELEGRAM_BOT_TOKEN is not set"
    exit 1
end
ok "Environment variables present"

# ── 2. Check Python venv ─────────────────────────────────────────────────────
if not test -f $VENV_PYTHON
    info "Creating Python venv..."
    uv venv $SCRIPT_DIR/.venv
    uv pip install fastapi uvicorn httpx pytest pytest-asyncio --python $VENV_PYTHON
end
ok "Python venv ready"

# ── 3. Write test config ──────────────────────────────────────────────────────
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
        "type": "telegram",
        "settings": {
          "token_env": "TELEGRAM_BOT_TOKEN"
        },
        "allowed_senders": []
      }
    ]
  }
}' > $CONFIG_FILE
ok "Config written to $CONFIG_FILE"

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

# ── 5. Start mock server ──────────────────────────────────────────────────────
set -x PYTHONPATH $SCRIPT_DIR
$VENV_PYTHON -c "
import time, signal, sys
from mock_tg import MockTelegramServer
server = MockTelegramServer(port=$MOCK_PORT)
server.start_background()
print('ready', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
" &
set MOCK_PID $last_pid

# Wait for health check
sleep 1
if not $VENV_PYTHON -c "
import httpx, sys
try:
    r = httpx.get('http://127.0.0.1:$MOCK_PORT/health', timeout=3)
    sys.exit(0 if r.status_code == 200 else 1)
except Exception as e:
    print(e); sys.exit(1)
" 2>/dev/null
    err "Mock server failed to start"
    lsof -i tcp:$MOCK_PORT
    exit 1
end
ok "Mock server running on port $MOCK_PORT (PID $MOCK_PID)"

# ── 6. Build bot ──────────────────────────────────────────────────────────────
section "Building octos (telegram feature)"
info "This may take a moment on first build..."
set BUILD_LOG /tmp/octos_build.log
cargo build --manifest-path $PROJECT_ROOT/Cargo.toml --bin octos --features telegram > $BUILD_LOG 2>&1
if test $status -ne 0
    err "Build failed:"
    cat $BUILD_LOG
    kill $MOCK_PID 2>/dev/null
    exit 1
end
ok "Build complete"

# ── 7. Start bot ──────────────────────────────────────────────────────────────
section "Starting octos gateway"
set -x TELOXIDE_API_URL http://127.0.0.1:$MOCK_PORT
rm -f $BOT_LOG
$BOT_BIN gateway --config $CONFIG_FILE > $BOT_LOG 2>&1 &
set BOT_PID $last_pid
info "Bot PID: $BOT_PID  |  Log: $BOT_LOG"

# Poll for "Gateway ready" with live log tail
echo ""
echo -e "$GRAY  Waiting for gateway to start...$RESET"
set READY 0
for i in (seq 1 40)
    sleep 1
    # Print any new log lines
    if test -f $BOT_LOG
        set LINES (cat $BOT_LOG | wc -l | string trim)
        if test $LINES -gt 0
            tail -1 $BOT_LOG | read LAST_LINE
            echo -e "$GRAY  › $LAST_LINE$RESET"
        end
    end
    if grep -q "gateway.*ready\|Gateway ready\|\[gateway\] ready" $BOT_LOG 2>/dev/null
        set READY 1
        break
    end
    # Detect early failure
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
ok "Gateway ready!"

# ── 8. Run tests ──────────────────────────────────────────────────────────────
section "Running tests"

set -x PYTHONPATH $SCRIPT_DIR
set -x MOCK_BASE_URL http://127.0.0.1:$MOCK_PORT

$VENV_PYTHON -m pytest $SCRIPT_DIR/test_bot.py -v --tb=short --no-header

set TEST_EXIT $status

# ── 9. Cleanup ────────────────────────────────────────────────────────────────
section "Cleanup"
kill $BOT_PID 2>/dev/null
kill $MOCK_PID 2>/dev/null
ok "Processes stopped"

echo ""
if test $TEST_EXIT -eq 0
    echo -e "$BOLD$GREEN  🎉 All tests passed!$RESET"
else
    echo -e "$BOLD$RED  💥 Some tests failed$RESET"
    echo -e "$GRAY  Bot log: $BOT_LOG$RESET"
end
echo ""
exit $TEST_EXIT
