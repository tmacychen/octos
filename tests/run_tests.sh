#!/usr/bin/env bash
# Octos Test Runner — unified entry point
#
# Usage:
#   tests/run_tests.sh                              # show help
#   tests/run_tests.sh all                          # run all tests (bot + cli)
#   tests/run_tests.sh --test bot [bot-args...]     # run bot mock tests
#   tests/run_tests.sh --test cli [cli-args...]     # run CLI tests
#
# Bot test arguments (after --test bot):
#   all              Run all bot modules (default)
#   telegram, tg     Run Telegram tests only
#   discord, dc      Run Discord tests only
#   list             List available bot modules
#   cases <mod>      List test cases in a module
#
# CLI test arguments (after --test cli):
#   -v, --verbose    Verbose output
#   -o, --output-dir Output directory (default: test-results)
#   -c, --config     Test config file (default: cli_test/test_cases.json)
#
# Before running any test, this script:
#   1. Creates a fixed test directory /tmp/octos_test
#   2. Compiles octos with ALL features into the test directory
#   3. Configures test config as needed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

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

info()  { echo -e "${CYAN}  ℹ ${RESET} $*" | tee -a "$SESSION_LOG"; }
ok()    { echo -e "${GREEN}  ✅ ${RESET} $*" | tee -a "$SESSION_LOG"; }
warn()  { echo -e "${YELLOW}  ⚠️  ${RESET} $*" | tee -a "$SESSION_LOG"; }
err()   { echo -e "${RED}  ❌ ${RESET} $*" | tee -a "$SESSION_LOG"; }
section() { echo "" | tee -a "$SESSION_LOG"; echo -e "${BOLD}${CYAN}── $* ${RESET}" | tee -a "$SESSION_LOG"; }
log_line() { echo -e "${GRAY}    $*${RESET}" | tee -a "$SESSION_LOG"; }

# Write a line to session log only (no stdout)
log_only() { echo "$*" >> "$SESSION_LOG"; }

# ── State ────────────────────────────────────────────────────────────────────
TEST_DIR="/tmp/octos_test"
LOG_DIR="$TEST_DIR/logs"
OCTOS_BIN="$TEST_DIR/octos"
CHECKSUM_FILE="$TEST_DIR/.octos_checksum"
SOURCE_BIN="$PROJECT_ROOT/target/debug/octos"
FAILED=0
MODULE_RESULTS=()   # tracks "module_name:PASS" or "module_name:FAIL"

# ── Session log ─────────────────────────────────────────────────────────────
SESSION_LOG_DIR="$LOG_DIR/sessions"
mkdir -p "$SESSION_LOG_DIR"
SESSION_LOG="$SESSION_LOG_DIR/$(date +%Y%m%d_%H%M%S).log"

# ── Binary sync with checksum ───────────────────────────────────────────────
sync_binary() {
    local src="$1"
    local dest="$2"
    local checksum_file="$3"

    if [[ ! -f "$src" ]]; then
        err "Source binary not found: $src"
        exit 1
    fi

    local new_cksum
    new_cksum=$(shasum -a 256 "$src" | cut -d' ' -f1)

    if [[ -f "$dest" ]] && [[ -f "$checksum_file" ]]; then
        local old_cksum
        old_cksum=$(cat "$checksum_file")
        if [[ "$new_cksum" == "$old_cksum" ]]; then
            ok "Binary unchanged (checksum match), skip copy"
            return 0
        fi
        warn "Binary changed, updating..."
    fi

    cp "$src" "$dest"
    chmod +x "$dest"
    echo "$new_cksum" > "$checksum_file"
    ok "Binary copied to $dest (checksum: ${new_cksum:0:12}...)"
}

# ── Build ────────────────────────────────────────────────────────────────────
build_octos() {
    section "Building octos (all features)"

    mkdir -p "$TEST_DIR" "$LOG_DIR"
    info "Test directory: $TEST_DIR"

    local build_log="$LOG_DIR/build.log"
    info "This may take a moment on first build..."
    info "Build log: $build_log"

    if ! cargo build \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        --bin octos \
        --all-features \
        2>&1 | tee "$build_log" | tee -a "$SESSION_LOG"; then
        err "Build failed (see log: $build_log)"
        exit 1
    fi
    ok "Build complete (all features)"

    sync_binary "$SOURCE_BIN" "$OCTOS_BIN" "$CHECKSUM_FILE"
}

# ── Bot test runner ─────────────────────────────────────────────────────────
run_bot_tests() {
    section "Running Bot Mock Tests"

    export OCTOS_TEST_DIR="$TEST_DIR"
    export OCTOS_LOG_DIR="$LOG_DIR"
    export OCTOS_BIN="$OCTOS_BIN"

    local bot_script="$SCRIPT_DIR/bot_mock_test/bot_test.sh"
    if [[ ! -f "$bot_script" ]]; then
        err "Bot test script not found: $bot_script"
        FAILED=1
        return
    fi

    # Pass sub-args to bot script (default: all)
    local bot_args=("$@")
    if [[ ${#bot_args[@]} -eq 0 ]]; then
        bot_args=("all")
    fi

    bash "$bot_script" "${bot_args[@]}" 2>&1 | tee -a "$SESSION_LOG"
    local bot_exit=${PIPESTATUS[0]}
    if [[ $bot_exit -ne 0 ]]; then
        err "Bot tests failed"
        MODULE_RESULTS+=("bot:FAIL")
        FAILED=1
    else
        ok "Bot tests passed"
        MODULE_RESULTS+=("bot:PASS")
    fi
}

# ── CLI test runner ─────────────────────────────────────────────────────────
run_cli_tests() {
    section "Running CLI Tests"

    local cli_script="$SCRIPT_DIR/cli_test/cli_test.sh"
    if [[ ! -f "$cli_script" ]]; then
        err "CLI test script not found: $cli_script"
        FAILED=1
        return
    fi

    export OCTOS_TEST_DIR="$TEST_DIR"
    export OCTOS_LOG_DIR="$LOG_DIR"

    # Always pass -b with binary path, forward remaining sub-args
    bash "$cli_script" -b "$OCTOS_BIN" "$@" 2>&1 | tee -a "$SESSION_LOG"
    local cli_exit=${PIPESTATUS[0]}
    if [[ $cli_exit -ne 0 ]]; then
        err "CLI tests failed"
        MODULE_RESULTS+=("cli:FAIL")
        FAILED=1
    else
        ok "CLI tests passed"
        MODULE_RESULTS+=("cli:PASS")
    fi
}

# ── Help ─────────────────────────────────────────────────────────────────────
show_help() {
    echo ""
    echo -e "${BOLD}  Octos Test Runner${RESET}"
    echo ""
    echo "  Usage:"
    echo "    tests/run_tests.sh <command> [args...]"
    echo ""
    echo "  Commands:"
    echo "    all                          Run all test suites (bot + cli)"
    echo "    --test bot [bot-args...]     Run bot mock tests"
    echo "    --test cli [cli-args...]     Run CLI tests"
    echo "    -h, --help                   Show this help message"
    echo ""
    echo "  Bot test arguments (after --test bot):"
    echo "    all              Run all bot modules (default)"
    echo "    telegram, tg     Run Telegram tests only"
    echo "    discord, dc      Run Discord tests only"
    echo "    list             List available bot modules"
    echo "    cases <mod>      List test cases in a module"
    echo ""
    echo "  CLI test arguments (after --test cli):"
    echo "    -v, --verbose    Verbose output"
    echo "    -o, --output-dir Output directory (default: test-results)"
    echo "    -c, --config     Test config file"
    echo ""
    echo "  Examples:"
    echo "    tests/run_tests.sh all                     # run everything"
    echo "    tests/run_tests.sh --test bot              # all bot tests"
    echo "    tests/run_tests.sh --test bot telegram     # Telegram only"
    echo "    tests/run_tests.sh --test bot cases tg     # list Telegram cases"
    echo "    tests/run_tests.sh --test cli              # CLI tests"
    echo "    tests/run_tests.sh --test cli -v           # CLI tests, verbose"
    echo ""
    echo "  Environment:"
    echo "    ANTHROPIC_API_KEY    Required for bot LLM tests"
    echo "    TELEGRAM_BOT_TOKEN   Required for Telegram bot tests"
    echo "    DISCORD_BOT_TOKEN    Optional (auto-set for mock mode)"
    echo ""
    echo "  Test directory: $TEST_DIR"
    echo "  Logs: $LOG_DIR/"
    echo ""
}

# ── Main ─────────────────────────────────────────────────────────────────────

ACTION="${1:-}"

if [[ -z "$ACTION" ]] || [[ "$ACTION" == "-h" ]] || [[ "$ACTION" == "--help" ]]; then
    show_help
    exit 0
fi

# ── Validate command early (before building) ─────────────────────────────────
case "$ACTION" in
    all|--test)
        # Valid commands, continue to env check and build
        ;;
    *)
        err "Unknown command: $ACTION"
        show_help
        exit 1
        ;;
esac

# ── Pre-flight env check ──────────────────────────────────────────────────────
check_bot_env() {
    local missing=()
    [[ -z "${ANTHROPIC_API_KEY:-}" ]]   && missing+=("ANTHROPIC_API_KEY")
    [[ -z "${TELEGRAM_BOT_TOKEN:-}" ]]  && missing+=("TELEGRAM_BOT_TOKEN")
    # DISCORD_BOT_TOKEN is optional (auto-set in mock mode)

    if [[ ${#missing[@]} -gt 0 ]]; then
        section "Missing required environment variables"
        for var in "${missing[@]}"; do
            err "$var is not set"
        done
        echo ""
        echo -e "  ${YELLOW}Set them before running, e.g.:${RESET}"
        echo -e "  ${GRAY}export ANTHROPIC_API_KEY=sk-...${RESET}"
        echo -e "  ${GRAY}export TELEGRAM_BOT_TOKEN=123456:ABC...${RESET}"
        echo ""
        exit 1
    fi
}

# Check env vars before building (saves time if they're missing)
case "$ACTION" in
    all)         check_bot_env ;;
    --test)
        TEST_TARGET="${2:-}"
        if [[ "$TEST_TARGET" == "bot" ]]; then check_bot_env; fi
        ;;
esac

# Build octos once (all features)
build_octos

case "$ACTION" in
    all)
        section "Running ALL test suites"
        run_bot_tests
        run_cli_tests
        ;;
    --test)
        TEST_TARGET="${2:-}"
        if [[ -z "$TEST_TARGET" ]]; then
            err "--test requires an argument: bot | cli"
            show_help
            exit 1
        fi
        shift 2  # remove --test and target, remaining args are sub-args
        case "$TEST_TARGET" in
            bot)  run_bot_tests "$@" ;;
            cli)  run_cli_tests "$@" ;;
            *)
                err "Unknown test target: $TEST_TARGET"
                echo "  Available: bot, cli"
                exit 1
                ;;
        esac
        ;;
esac

# ── Final summary ────────────────────────────────────────────────────────────
echo ""
if [[ $FAILED -eq 0 ]]; then
    echo -e "${BOLD}${GREEN}  🎉 All tests passed!${RESET}" | tee -a "$SESSION_LOG"
else
    echo -e "${BOLD}${RED}  💥 Some tests failed${RESET}" | tee -a "$SESSION_LOG"
fi

# ── Test summary ────────────────────────────────────────────────────────────
section "Test Summary"
echo -e "  Date:    $(date '+%Y-%m-%d %H:%M:%S')" | tee -a "$SESSION_LOG"
echo -e "  Result:  $([ $FAILED -eq 0 ] && echo 'PASSED' || echo 'FAILED')" | tee -a "$SESSION_LOG"
echo -e "  Modules:" | tee -a "$SESSION_LOG"
for result in "${MODULE_RESULTS[@]}"; do
    mod_name="${result%%:*}"
    mod_status="${result##*:}"
    if [[ "$mod_status" == "PASS" ]]; then
        echo -e "    ${GREEN}✅ ${mod_name}${RESET}" | tee -a "$SESSION_LOG"
    else
        echo -e "    ${RED}❌ ${mod_name}${RESET}" | tee -a "$SESSION_LOG"
    fi
done
echo -e "  Log:     $SESSION_LOG" | tee -a "$SESSION_LOG"
echo ""

exit $FAILED
