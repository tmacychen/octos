#!/usr/bin/env bash
# Bot Mock Test Runner — invoked by run_tests.sh
#
# Do NOT run this script directly. Use:
#   tests/run_tests.sh --test bot [args...]
#
# Bot test arguments:
#   all              Run all bot modules (default)
#   telegram, tg     Run Telegram tests only
#   discord, dc      Run Discord tests only
#   list             List available bot modules
#   cases <mod>      List test cases in a module

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ── Must be invoked from run_tests.sh ────────────────────────────────────────
if [[ -z "${OCTOS_BIN:-}" ]]; then
    echo ""
    echo -e "\033[0;31m  ❌ This script cannot be run directly.\033[0m"
    echo ""
    echo "  Please use the unified test runner:"
    echo ""
    echo "    tests/run_tests.sh --test bot [args...]"
    echo ""
    echo "  Available args: all | telegram | tg | discord | dc | list | cases <mod>"
    echo ""
    exit 1
fi

BOT_BIN="$OCTOS_BIN"
TEST_DIR="${OCTOS_TEST_DIR:-/tmp/octos_test}"
LOG_DIR="${OCTOS_LOG_DIR:-$TEST_DIR/logs}"

VENV_PYTHON="$SCRIPT_DIR/.venv/bin/python"

# ── Available modules ────────────────────────────────────────────────────────
# Each module: name|alias|port|test_file|feature|mock_module|mock_class
MODULES=(
    "telegram|tg|5000|test_telegram.py|telegram|mock_tg|MockTelegramServer"
    "discord|dc|5001|test_discord.py|discord|mock_discord|MockDiscordServer"
)

# ── Colors ───────────────────────────────────────────────────────────────────
if [[ -t 1 ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    GRAY='\033[0;90m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' GRAY='' BOLD='' RESET=''
fi

info()    { echo -e "${CYAN}  ℹ ${RESET} $*"; }
ok()      { echo -e "${GREEN}  ✅ ${RESET} $*"; }
warn()    { echo -e "${YELLOW}  ⚠️  ${RESET} $*"; }
err()     { echo -e "${RED}  ❌ ${RESET} $*"; }
section() { echo ""; echo -e "${BOLD}${CYAN}── $* ${RESET}"; }
log_line() { echo -e "${GRAY}    $*${RESET}"; }

# ── Module helpers ───────────────────────────────────────────────────────────

list_modules() {
    echo ""
    echo -e "${BOLD}  Available test modules:${RESET}"
    echo ""
    for mod in "${MODULES[@]}"; do
        IFS='|' read -r name alias port testf _ _ _ <<< "$mod"
        echo -e "  ${GREEN}${name}${RESET} (${alias})  —  port ${port}, test file: ${testf}"
    done
    echo ""
}

list_cases() {
    local mod="$1"
    parse_module "$mod"

    if [[ ! -f "$VENV_PYTHON" ]]; then
        err "Python venv not found. Run a test first to create it."
        return 1
    fi

    if [[ ! -f "$SCRIPT_DIR/$MOD_TEST_FILE" ]]; then
        err "Test file not found: $SCRIPT_DIR/$MOD_TEST_FILE"
        return 1
    fi

    echo ""
    echo -e "${BOLD}  Test cases in ${MOD_NAME} (${MOD_TEST_FILE}):${RESET}"
    echo ""

    local cases
    cases=$(PYTHONPATH="$SCRIPT_DIR" "$VENV_PYTHON" -m pytest "$SCRIPT_DIR/$MOD_TEST_FILE" \
        --collect-only -q --no-header 2>/dev/null | grep "::" | sed 's/^.*:://')

    local current_class=""
    local idx=0
    while IFS= read -r c; do
        [[ "$c" != *"test_"* ]] && continue
        [[ "$c" == *"Warning"* ]] && continue
        [[ "$c" == *"pytest"* ]] && continue

        if [[ "$c" == *"::"* ]]; then
            local cls func
            IFS='::' read -r cls func <<< "$c"
            if [[ "$cls" != "$current_class" ]]; then
                current_class="$cls"
                echo -e "  ${CYAN}${cls}${RESET}"
            fi
            idx=$((idx + 1))
            echo -e "    ${GRAY}${idx}${RESET}  ${GREEN}${func}${RESET}"
        else
            idx=$((idx + 1))
            echo -e "    ${GRAY}${idx}${RESET}  ${GREEN}${c}${RESET}"
        fi
    done <<< "$cases"

    if [[ $idx -eq 0 ]]; then
        warn "No test cases found"
    else
        echo ""
        echo -e "  ${GRAY}${idx} test(s) in total${RESET}"
    fi
    echo ""
}

resolve_module() {
    local query="$1"
    for mod in "${MODULES[@]}"; do
        IFS='|' read -r name alias _ _ _ _ _ <<< "$mod"
        if [[ "$query" == "$name" ]] || [[ "$query" == "$alias" ]]; then
            echo "$mod"
            return 0
        fi
    done
    return 1
}

# Global variables set by parse_module
MOD_NAME=""
MOD_ALIAS=""
MOD_PORT=""
MOD_TEST_FILE=""
MOD_FEATURE=""
MOD_MOCK_MODULE=""
MOD_MOCK_CLASS=""
TEST_CASE=""  # Optional: specific test case to run

parse_module() {
    local mod="$1"
    IFS='|' read -r MOD_NAME MOD_ALIAS MOD_PORT MOD_TEST_FILE MOD_FEATURE MOD_MOCK_MODULE MOD_MOCK_CLASS <<< "$mod"
}

# ── Per-module setup functions ───────────────────────────────────────────────
# These set: BOT_LOG, CONFIG_FILE, EXTRA_ENV_VAR, EXTRA_ENV_VAL, CONFIG_JSON

BOT_LOG=""
CONFIG_FILE=""
EXTRA_ENV_VAR=""
EXTRA_ENV_VAL=""
CONFIG_JSON=""

setup_telegram_env() {
    if [[ -z "${TELEGRAM_BOT_TOKEN:-}" ]]; then
        err "TELEGRAM_BOT_TOKEN is not set"
        return 1
    fi
    BOT_LOG="$LOG_DIR/octos_telegram_bot_test_$(date +%Y%m%d_%H%M%S).log"
    CONFIG_FILE="$TEST_DIR/.octos/test_telegram_config.json"
    EXTRA_ENV_VAR="TELOXIDE_API_URL"
    EXTRA_ENV_VAL="http://127.0.0.1:$MOD_PORT"
    CONFIG_JSON='{
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
    ],
    "queue_mode": "interrupt"
  }
}'
    return 0
}

setup_discord_env() {
    if [[ -z "${DISCORD_BOT_TOKEN:-}" ]]; then
        export DISCORD_BOT_TOKEN="mock-bot-token-for-testing"
        info "DISCORD_BOT_TOKEN not set, using dummy value (mock mode)"
    fi
    BOT_LOG="$LOG_DIR/octos_discord_bot_test_$(date +%Y%m%d_%H%M%S).log"
    CONFIG_FILE="$TEST_DIR/.octos/test_discord_config.json"
    EXTRA_ENV_VAR="DISCORD_API_BASE_URL"
    EXTRA_ENV_VAL="http://127.0.0.1:$MOD_PORT"
    CONFIG_JSON='{
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
    ],
    "queue_mode": "collect"
  }
}'
    return 0
}

# ── Core runner ──────────────────────────────────────────────────────────────

run_module() {
    local mod="$1"
    parse_module "$mod"

    # Setup cleanup trap to kill processes on exit/interrupt
    # Use ${VAR:-} to avoid unbound variable errors with set -u
    trap 'kill ${BOT_PID:-} ${MOCK_PID:-} 2>/dev/null; wait ${BOT_PID:-} ${MOCK_PID:-} 2>/dev/null || true' EXIT INT TERM

    section "Running $MOD_NAME tests (port $MOD_PORT)"

    # ── 0. Print octos version ──────────────────────────────────────────────
    local octos_version
    octos_version=$("$BOT_BIN" --version 2>/dev/null || echo "unknown")
    info "octos version: $octos_version"

    # ── 1. Ensure log directory exists ───────────────────────────────────────
    mkdir -p "$LOG_DIR"

    # ── 1. Check ANTHROPIC_API_KEY ───────────────────────────────────────────
    if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
        err "ANTHROPIC_API_KEY is not set"
        return 1
    fi

    # ── 2. Module-specific env setup ─────────────────────────────────────────
    case "$MOD_NAME" in
        telegram)
            if ! setup_telegram_env; then
                return 1
            fi
            ;;
        discord)
            setup_discord_env
            ;;
    esac
    ok "Environment variables present"

    # ── 3. Check Python venv ─────────────────────────────────────────────────
    if [[ ! -f "$VENV_PYTHON" ]]; then
        info "Creating Python venv..."
        uv venv "$SCRIPT_DIR/.venv"
        uv pip install fastapi uvicorn httpx pytest pytest-asyncio websockets --python "$VENV_PYTHON"
    fi
    ok "Python venv ready"

    # ── 4. Write test config ─────────────────────────────────────────────────
    section "Writing config"
    mkdir -p "$(dirname "$CONFIG_FILE")"
    echo "$CONFIG_JSON" > "$CONFIG_FILE"
    ok "Config written to $CONFIG_FILE ($MOD_NAME channel)"

    # ── 5. Kill anything on the mock port ────────────────────────────────────
    section "Preparing mock server"
    
    # Kill all Mock Server processes (not just those on the port)
    local mock_pids
    mock_pids=$(ps aux | grep -E "mock_tg|mock_discord|MockTelegramServer|MockDiscordServer" | grep -v grep | awk '{print $2}' || true)
    if [[ -n "$mock_pids" ]]; then
        warn "Force killing existing Mock Server processes: $mock_pids"
        echo "$mock_pids" | xargs kill -9 2>/dev/null || true
        sleep 1
    fi
    
    # Also kill anything on the mock port (fallback)
    local existing_pid
    existing_pid=$(lsof -ti tcp:"$MOD_PORT" 2>/dev/null || true)
    if [[ -n "$existing_pid" ]]; then
        warn "Port $MOD_PORT still in use by PID $existing_pid, force killing..."
        kill -9 "$existing_pid" 2>/dev/null || true
        for _ in $(seq 1 10); do
            sleep 0.5
            if ! lsof -ti tcp:"$MOD_PORT" >/dev/null 2>&1; then
                break
            fi
        done
    fi

    # 🔥 CRITICAL: Clear Python cache to ensure fresh code is loaded
    # This prevents stale .pyc files from causing issues after code changes
    info "Clearing Python cache..."
    
    # Count files before deletion for logging
    local pycache_count=$(find "$SCRIPT_DIR" -type d -name "__pycache__" 2>/dev/null | wc -l | tr -d ' ')
    local pyc_count=$(find "$SCRIPT_DIR" -name "*.pyc" 2>/dev/null | wc -l | tr -d ' ')
    local pytest_cache_exists=0
    [[ -d "$SCRIPT_DIR/.pytest_cache" ]] && pytest_cache_exists=1
    
    # Clear __pycache__ directories
    if [[ $pycache_count -gt 0 ]]; then
        find "$SCRIPT_DIR" -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
        info "  Removed $pycache_count __pycache__ directories"
    fi
    
    # Clear .pyc files
    if [[ $pyc_count -gt 0 ]]; then
        find "$SCRIPT_DIR" -name "*.pyc" -delete 2>/dev/null || true
        info "  Deleted $pyc_count .pyc files"
    fi
    
    # Clear pytest cache
    if [[ $pytest_cache_exists -eq 1 ]]; then
        rm -rf "$SCRIPT_DIR/.pytest_cache" 2>/dev/null || true
        info "  Removed .pytest_cache directory"
    fi
    
    # 🔥 CRITICAL FIX: Clear uv/virtualenv import cache
    # uv caches compiled bytecode in site-packages/__pycache__
    if [[ -d "$SCRIPT_DIR/.venv" ]]; then
        local venv_pycache_count=$(find "$SCRIPT_DIR/.venv/lib" -type d -name "__pycache__" 2>/dev/null | wc -l | tr -d ' ')
        local venv_pyc_count=$(find "$SCRIPT_DIR/.venv/lib" -name "*.pyc" 2>/dev/null | wc -l | tr -d ' ')
        
        if [[ $venv_pycache_count -gt 0 ]]; then
            find "$SCRIPT_DIR/.venv/lib" -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
            info "  Removed $venv_pycache_count venv __pycache__ directories"
        fi
        
        if [[ $venv_pyc_count -gt 0 ]]; then
            find "$SCRIPT_DIR/.venv/lib" -name "*.pyc" -delete 2>/dev/null || true
            info "  Deleted $venv_pyc_count venv .pyc files"
        fi
    fi
    
    ok "Cache cleared (total: $((pycache_count + pyc_count + pytest_cache_exists)) items)"

    # ── 6. Start mock server ─────────────────────────────────────────────────
    # 🔥 CRITICAL FIX: Force delete all Python cache before starting Mock Server
    # This ensures the latest code changes are loaded
    info "Force clearing all Python cache before Mock Server startup..."
    find "$SCRIPT_DIR" -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
    find "$SCRIPT_DIR" -name "*.pyc" -delete 2>/dev/null || true
    ok "Cache cleared"
    
    # Start mock server
    local health_timeout=3
    if [[ "$MOD_NAME" == "discord" ]]; then
        health_timeout=5
    fi

    PYTHONPATH="$SCRIPT_DIR" PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 "$VENV_PYTHON" -c "
import time, signal, sys
from ${MOD_MOCK_MODULE} import ${MOD_MOCK_CLASS}
server = ${MOD_MOCK_CLASS}(port=${MOD_PORT})
# Pass log file to redirect all uvicorn output
server.start_background(log_file='$BOT_LOG')
print('ready', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
" >> "$BOT_LOG" 2>&1 &
    MOCK_PID=$!

    local wait_sec=1
    if [[ "$MOD_NAME" == "discord" ]]; then
        wait_sec=2
    fi
    sleep "$wait_sec"

    if ! "$VENV_PYTHON" -c "
import httpx, sys
try:
    r = httpx.get('http://127.0.0.1:${MOD_PORT}/health', timeout=${health_timeout})
    sys.exit(0 if r.status_code == 200 else 1)
except Exception as e:
    print(e); sys.exit(1)
" 2>/dev/null; then
        err "$MOD_NAME Mock server failed to start"
        lsof -i tcp:"$MOD_PORT"
        return 1
    fi
    ok "$MOD_NAME Mock server running on port $MOD_PORT (PID $MOCK_PID)"

    # ── 7. Start bot ─────────────────────────────────────────────────────────
    section "Starting octos gateway"
    if [[ ! -x "$BOT_BIN" ]]; then
        err "octos binary not found: $BOT_BIN"
        kill "$MOCK_PID" 2>/dev/null || true
        return 1
    fi
    rm -f "$BOT_LOG"

    export "$EXTRA_ENV_VAR"="$EXTRA_ENV_VAL"
    # octos gateway output to log file only (not stdout)
    "$BOT_BIN" gateway --config "$CONFIG_FILE" --data-dir "$TEST_DIR" >> "$BOT_LOG" 2>&1 &
    BOT_PID=$!
    info "Bot PID: $BOT_PID"

    # Poll for ready
    echo ""
    echo -e "${GRAY}  Waiting for gateway to start...${RESET}"
    READY=0
    MAX_WAIT=40
    if [[ "$MOD_NAME" == "discord" ]]; then
        MAX_WAIT=50
    fi
    for _ in $(seq 1 "$MAX_WAIT"); do
        sleep 1
        if [[ -f "$BOT_LOG" ]]; then
            local lines
            lines=$(wc -l < "$BOT_LOG" | tr -d ' ')
            if [[ "$lines" -gt 0 ]]; then
                local last_line
                last_line=$(tail -1 "$BOT_LOG")
                echo -e "${GRAY}  › ${last_line}${RESET}"
            fi
        fi
        if grep -q "gateway.*ready\|Gateway ready\|\[gateway\] ready" "$BOT_LOG" 2>/dev/null; then
            READY=1
            break
        fi
        if [[ "$MOD_NAME" == "discord" ]] && grep -q "Discord.*bot connected\|Discord channel started" "$BOT_LOG" 2>/dev/null; then
            READY=1
            break
        fi
        if grep -q "^Error:" "$BOT_LOG" 2>/dev/null; then
            break
        fi
    done

    if [[ $READY -eq 0 ]]; then
        err "Bot failed to start. Full log:"
        echo ""
        while IFS= read -r line; do
            log_line "$line"
        done < "$BOT_LOG"
        echo ""
        kill "$BOT_PID" 2>/dev/null || true
        kill "$MOCK_PID" 2>/dev/null || true
        return 1
    fi
    ok "Gateway ready! ($MOD_NAME channel active)"

    # ── 8. Run tests ─────────────────────────────────────────────────────────
    section "Running $MOD_NAME tests"

    export PYTHONPATH="$SCRIPT_DIR"
    export MOCK_BASE_URL="http://127.0.0.1:$MOD_PORT"

    # 🔥 CRITICAL: Clear cache again before running tests to ensure latest code
    # Mock server startup may have regenerated stale .pyc files
    info "Clearing cache before test execution..."
    find "$SCRIPT_DIR" -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
    find "$SCRIPT_DIR" -name "*.pyc" -delete 2>/dev/null || true
    rm -rf "$SCRIPT_DIR/.pytest_cache" 2>/dev/null || true
    ok "Cache cleared before tests"

    # Build pytest command (no separate log file, output captured by tee)
    export PYTHONDONTWRITEBYTECODE=1
    local pytest_cmd=("$VENV_PYTHON" -m pytest "$SCRIPT_DIR/$MOD_TEST_FILE" \
        -v --tb=short --no-header --color=yes \
        --capture=no \
        --log-cli-level=INFO)
    
    # If specific test case provided, add it to the command
    if [[ -n "$TEST_CASE" ]]; then
        pytest_cmd+=("-k" "$TEST_CASE")
        info "Running specific test: $TEST_CASE"
    fi

    # Run tests - output goes to stdout AND log file via tee
    info "All test output will be saved to: $BOT_LOG"
    "${pytest_cmd[@]}" 2>&1 | tee -a "$BOT_LOG"
    local test_exit=${PIPESTATUS[0]}

    # ── 9. Cleanup ───────────────────────────────────────────────────────────
    section "Cleanup"
    
    # Kill processes and wait for them to exit
    kill "$BOT_PID" 2>/dev/null || true
    kill "$MOCK_PID" 2>/dev/null || true
    
    # Wait for processes to terminate (with timeout)
    for i in {1..10}; do
        if ! kill -0 "$BOT_PID" 2>/dev/null && ! kill -0 "$MOCK_PID" 2>/dev/null; then
            break
        fi
        sleep 0.5
    done
    
    # Force kill if still running
    kill -9 "$BOT_PID" 2>/dev/null || true
    kill -9 "$MOCK_PID" 2>/dev/null || true
    
    ok "Processes stopped"

    echo ""
    if [[ $test_exit -eq 0 ]]; then
        echo -e "${BOLD}${GREEN}  🎉 All $MOD_NAME tests passed!${RESET}"
    else
        echo -e "${BOLD}${RED}  💥 Some $MOD_NAME tests failed${RESET}"
        echo -e "${GRAY}  Bot log: $BOT_LOG${RESET}"
    fi
    echo ""

    return $test_exit
}

# ── Main ─────────────────────────────────────────────────────────────────────

ACTION="${1:-all}"

case "$ACTION" in
    -h|--help)
        echo ""
        echo -e "${BOLD}  Bot Mock Test Runner${RESET}"
        echo ""
        echo "  Do NOT run directly. Use:"
        echo "    tests/run_tests.sh --test bot [args...]"
        echo ""
        echo "  Arguments:"
        echo "    all              Run all bot modules (default)"
        echo "    telegram, tg     Run Telegram tests"
        echo "    discord, dc      Run Discord tests"
        echo "    list             List available modules"
        echo "    cases <mod>      List test cases in a module"
        echo "    <mod> [case]     Run module or specific test case"
        echo ""
        echo "  Examples:"
        echo "    tests/run_tests.sh --test bot telegram"
        echo "    tests/run_tests.sh --test bot telegram test_concurrent_session_creation"
        echo ""
        exit 0
        ;;
    list|ls)
        list_modules
        exit 0
        ;;
    cases)
        target="${2:-}"
        if [[ -z "$target" ]]; then
            err "Usage: tests/run_tests.sh --test bot cases <module>"
            list_modules
            exit 1
        fi
        resolved=$(resolve_module "$target" || true)
        if [[ -z "$resolved" ]]; then
            err "Unknown module: $target"
            list_modules
            exit 1
        fi
        list_cases "$resolved"
        exit 0
        ;;
    all)
        section "Running ALL test modules"
        FAILED=0
        for mod in "${MODULES[@]}"; do
            if ! run_module "$mod"; then
                FAILED=1
            fi
        done
        if [[ $FAILED -eq 0 ]]; then
            echo -e "${BOLD}${GREEN}  🎉 All modules passed!${RESET}"
        else
            echo -e "${BOLD}${RED}  💥 Some modules failed${RESET}"
        fi
        exit $FAILED
        ;;
    *)
        # Smart detection: check if first arg is a module or test case
        target="${1:-}"
        TEST_CASE="${2:-}"
        
        if [[ -z "$target" ]]; then
            err "No module specified"
            list_modules
            exit 1
        fi
        
        # Try to resolve as module name
        resolved=$(resolve_module "$target" || true)
        
        if [[ -n "$resolved" ]]; then
            # It's a valid module, run it (with optional test case filter)
            if [[ -n "$TEST_CASE" ]]; then
                info "Running specific test case: $TEST_CASE in $target"
            fi
            run_module "$resolved"
            exit $?
        else
            # Not a module name, check if it looks like a test case name
            if [[ "$target" == test_* ]]; then
                err "Test case '$target' specified without module name"
                echo ""
                echo "  Usage: tests/run_tests.sh --test bot <module> <test_case>"
                echo "  Example: tests/run_tests.sh --test bot telegram $target"
                echo ""
                list_modules
                exit 1
            else
                err "Unknown module: $target"
                list_modules
                exit 1
            fi
        fi
        ;;
esac
