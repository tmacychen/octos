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

# ── State ────────────────────────────────────────────────────────────────────
TEST_DIR="/tmp/octos_test"
LOG_DIR="$TEST_DIR/logs"
OCTOS_BIN="$TEST_DIR/octos"
CHECKSUM_FILE="$TEST_DIR/.octos_checksum"
SOURCE_BIN="$PROJECT_ROOT/target/debug/octos"
FAILED=0
MODULE_RESULTS=()   # tracks "module_name:PASS" or "module_name:FAIL"

# ── Session log (must be defined before any logging functions) ───────────────
SESSION_LOG_DIR="$LOG_DIR/sessions"
mkdir -p "$SESSION_LOG_DIR"
SESSION_LOG="$SESSION_LOG_DIR/$(date +%Y%m%d_%H%M%S).log"

# ── Redirect all stdout/stderr to log file AND terminal (via tee) ────────────
# This ensures ALL output from this script is captured in the session log
exec > >(tee -a "$SESSION_LOG") 2>&1

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

info()  { echo -e "${CYAN}  ℹ ${RESET} $*"; }
ok()    { echo -e "${GREEN}  ✅ ${RESET} $*"; }
warn()  { echo -e "${YELLOW}  ⚠️  ${RESET} $*"; }
err()   { echo -e "${RED}  ❌ ${RESET} $*"; }
section() { echo ""; echo -e "${BOLD}${CYAN}── $* ${RESET}"; }
log_line() { echo -e "${GRAY}    $*${RESET}"; }

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
        2>&1 | tee "$build_log"; then
        err "Build failed (see log: $build_log)"
        exit 1
    fi
    ok "Build complete (all features)"

    sync_binary "$SOURCE_BIN" "$OCTOS_BIN" "$CHECKSUM_FILE"
}

# ── Bot test runner ─────────────────────────────────────────────────────────
run_bot_tests() {
    # Pass sub-args to bot script (default: all)
    local bot_args=("$@")
    if [[ ${#bot_args[@]} -eq 0 ]]; then
        bot_args=("all")
    fi

    # Check if first argument is a list operation (not a test run)
    local first_arg="${bot_args[0]:-all}"
    if [[ "$first_arg" == "list" ]] || [[ "$first_arg" == "cases" ]]; then
        # These are info commands, just delegate to bot_test.sh and return
        local bot_script="$SCRIPT_DIR/bot_mock_test/bot_test.sh"
        if [[ ! -f "$bot_script" ]]; then
            err "Bot test script not found: $bot_script"
            return 1
        fi
        # Set minimal env vars for list operations (no build needed)
        export OCTOS_TEST_DIR="$TEST_DIR"
        export OCTOS_LOG_DIR="$LOG_DIR"
        export OCTOS_BIN="${OCTOS_BIN:-placeholder}"
        bash "$bot_script" "${bot_args[@]}"
        return
    fi

    # Actual test execution
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

    # Determine which module(s) to test and set log file accordingly
    local timestamp=$(date +%Y%m%d_%H%M%S)
    local bot_log
    
    case "$first_arg" in
        telegram|tg)
            bot_log="$LOG_DIR/octos_telegram_bot_test_${timestamp}.log"
            ;;
        discord|dc)
            bot_log="$LOG_DIR/octos_discord_bot_test_${timestamp}.log"
            ;;
        all|*)
            bot_log="$LOG_DIR/octos_bot_test_${timestamp}.log"
            ;;
    esac

    info "Bot test log: $bot_log"
    
    # Redirect output to module-specific log file AND terminal
    bash "$bot_script" "${bot_args[@]}" 2>&1 | tee -a "$bot_log"
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

    # Check if this is a list operation (no build needed)
    local first_arg="${1:-}"
    if [[ "$first_arg" == "list" ]]; then
        export OCTOS_TEST_DIR="$TEST_DIR"
        export OCTOS_LOG_DIR="$LOG_DIR"
        bash "$cli_script" -b "${OCTOS_BIN:-placeholder}" "$@"
        return
    fi

    export OCTOS_TEST_DIR="$TEST_DIR"
    export OCTOS_LOG_DIR="$LOG_DIR"

    # Always pass -b with binary path, forward remaining sub-args
    # Output already redirected by exec, just capture exit code
    bash "$cli_script" -b "$OCTOS_BIN" "$@"
    local cli_exit=$?
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
    echo "    list <mod>       List test cases in a module"
    echo "    <mod> [case]     Run module or specific test case"
    echo ""
    echo "  CLI test arguments (after --test cli):"
    echo "    -v, --verbose              Verbose output"
    echo "    -o, --output-dir DIR       Output directory (default: test-results)"
    echo "    -s, --scope SCOPE          Test scope: all|CLI|Init|Clean|Status|Completions|Skills|Auth|Channels|Cron|Chat|Gateway|Serve|Docs"
    echo "    list                       List available test categories"
    echo "    list <category>            List test cases in a category"
    echo ""
    echo "  Examples:"
    echo "    tests/run_tests.sh all                     # run everything"
    echo "    tests/run_tests.sh --test bot              # all bot tests"
    echo "    tests/run_tests.sh --test bot telegram     # Telegram only"
    echo "    tests/run_tests.sh --test bot list         # list bot modules"
    echo "    tests/run_tests.sh --test bot list tg      # list Telegram test cases"
    echo "    tests/run_tests.sh --test bot list discord # list Discord test cases"
    echo "    tests/run_tests.sh --test bot tg           # run Telegram tests"
    echo "    tests/run_tests.sh --test bot tg test_concurrent_session_creation  # run single test"
    echo "    tests/run_tests.sh --test cli              # CLI tests"
    echo "    tests/run_tests.sh --test cli -v           # CLI tests, verbose"
    echo "    tests/run_tests.sh --test cli list         # List test categories"
    echo "    tests/run_tests.sh --test cli list Init    # List Init test cases"
    echo "    tests/run_tests.sh --test cli -s Init      # Run only Init tests"
    echo "    tests/run_tests.sh --test cli -s Completions # Run only Completions tests"
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
        # Valid commands, continue to validation
        ;;
    *)
        err "Unknown command: $ACTION"
        show_help
        exit 1
        ;;
esac

# ── Validate --test arguments before building ────────────────────────────────
if [[ "$ACTION" == "--test" ]]; then
    TEST_TARGET="${2:-}"
    if [[ -z "$TEST_TARGET" ]]; then
        err "--test requires an argument: bot | cli"
        show_help
        exit 1
    fi
    
    # Validate test target
    if [[ "$TEST_TARGET" != "bot" ]] && [[ "$TEST_TARGET" != "cli" ]]; then
        err "Unknown test target: $TEST_TARGET"
        echo ""
        echo "Available test targets:"
        echo "  bot    Run bot mock tests (Telegram, Discord)"
        echo "  cli    Run CLI tests"
        echo ""
        echo "Examples:"
        echo "  tests/run_tests.sh --test bot              # All bot tests"
        echo "  tests/run_tests.sh --test cli              # All CLI tests"
        exit 1
    fi
    
    # Pre-validate Bot test arguments (before building)
    if [[ "$TEST_TARGET" == "bot" ]] && [[ $# -ge 3 ]]; then
        valid_bot_args="all telegram tg discord dc list cases"
        first_bot_arg="${3:-}"
        
        # Check if first bot argument is valid
        if ! echo "$valid_bot_args" | grep -qw "$first_bot_arg"; then
            err "Invalid argument for Bot tests: $first_bot_arg"
            echo ""
            echo "This check runs before compilation to catch typos early."
            echo ""
            echo "Valid Bot test arguments:"
            echo "  all                  Run all bot modules (default)"
            echo "  telegram, tg         Run Telegram tests only"
            echo "  discord, dc          Run Discord tests only"
            echo "  list                 List available bot modules"
            echo "  list <mod>           List test cases in a module"
            echo ""
            echo "For general help, use: tests/run_tests.sh --help"
            echo ""
            echo "Examples:"
            echo "  tests/run_tests.sh --test bot              # Run all bot tests"
            echo "  tests/run_tests.sh --test bot telegram     # Run Telegram tests only"
            echo "  tests/run_tests.sh --test bot list         # List bot modules"
            echo "  tests/run_tests.sh --test bot list tg      # List Telegram test cases"
            exit 1
        fi
    fi
    
    # Pre-validate CLI test arguments (before building)
    # Only check for obviously invalid option flags, detailed validation is done by cli_test.sh
    if [[ "$TEST_TARGET" == "cli" ]] && [[ $# -ge 3 ]]; then
        valid_opts="-v --verbose -o --output-dir -s --scope list"
        prev_arg=""
        for i in $(seq 3 $#); do
            arg="${!i}"
            # Check if it's an option flag (starts with -) and not a value for previous option
            if [[ "$arg" == -* ]] && [[ "$prev_arg" != "-o" ]] && [[ "$prev_arg" != "--output-dir" ]] && \
               [[ "$prev_arg" != "-s" ]] && [[ "$prev_arg" != "--scope" ]]; then
                # It's an option flag, check if it's in the whitelist
                if ! echo "$valid_opts" | grep -q -- "$arg"; then
                    err "Invalid argument for CLI tests: $arg"
                    echo ""
                    echo "This check runs before compilation to catch typos early."
                    echo ""
                    echo "Valid CLI test options:"
                    echo "  -v, --verbose          Verbose output"
                    echo "  -o, --output-dir DIR   Output directory"
                    echo "  -s, --scope SCOPE      Test scope (all|CLI|Init|Clean|...)"
                    echo "  list                   List available test categories"
                    echo "  list <category>        List test cases in a category"
                    echo ""
                    echo "For general help, use: tests/run_tests.sh --help"
                    echo ""
                    echo "Examples:"
                    echo "  tests/run_tests.sh --test cli              # Run all CLI tests"
                    echo "  tests/run_tests.sh --test cli list         # List test categories"
                    echo "  tests/run_tests.sh --test cli list Init    # List Init test cases"
                    exit 1
                fi
                
                # Check if this option requires a value and if there's a next argument
                case "$arg" in
                    -o|--output-dir|-s|--scope)
                        next_i=$((i + 1))
                        if [[ $next_i -le $# ]]; then
                            next_arg="${!next_i}"
                            # If next arg starts with -, it's likely another option, not a value
                            if [[ "$next_arg" == -* ]]; then
                                err "Option $arg requires a value"
                                echo ""
                                echo "Example: tests/run_tests.sh --test cli -s Init"
                                exit 1
                            fi
                        else
                            err "Option $arg requires a value"
                            echo ""
                            echo "Example: tests/run_tests.sh --test cli -s Init"
                            exit 1
                        fi
                        ;;
                esac
            fi
            prev_arg="$arg"
        done
    fi
fi

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
# But skip for list/cases operations
case "$ACTION" in
    all)         check_bot_env ;;
    --test)
        TEST_TARGET="${2:-}"
        FIRST_SUB_ARG="${3:-}"  # First argument after --test <target>
        
        # Skip env check for list/cases operations
        if [[ "$FIRST_SUB_ARG" == "list" ]] || [[ "$FIRST_SUB_ARG" == "cases" ]]; then
            : # No env check needed
        elif [[ "$TEST_TARGET" == "bot" ]]; then
            check_bot_env
        fi
        ;;
esac

# Check if we need to build (skip for list/cases operations)
NEED_BUILD=true
case "$ACTION" in
    --test)
        TEST_TARGET="${2:-}"
        FIRST_SUB_ARG="${3:-}"  # First argument after --test <target>
        # Check if first sub-arg is a list operation
        if [[ "$FIRST_SUB_ARG" == "list" ]] || [[ "$FIRST_SUB_ARG" == "cases" ]]; then
            NEED_BUILD=false
        fi
        ;;
esac

# Build octos once (all features) - only if needed
if [[ "$NEED_BUILD" == true ]]; then
    build_octos
fi

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
    echo -e "${BOLD}${GREEN}  🎉 All tests passed!${RESET}"
else
    echo -e "${BOLD}${RED}  💥 Some tests failed${RESET}"
fi

# ── Test summary ────────────────────────────────────────────────────────────
section "Test Summary"
echo -e "  Date:    $(date '+%Y-%m-%d %H:%M:%S')"
echo -e "  Result:  $([ $FAILED -eq 0 ] && echo 'PASSED' || echo 'FAILED')"
echo -e "  Modules:"
if [[ ${#MODULE_RESULTS[@]} -gt 0 ]]; then
    for result in "${MODULE_RESULTS[@]}"; do
        mod_name="${result%%:*}"
        mod_status="${result##*:}"
        if [[ "$mod_status" == "PASS" ]]; then
            echo -e "    ${GREEN}✅ ${mod_name}${RESET}"
        else
            echo -e "    ${RED}❌ ${mod_name}${RESET}"
        fi
    done
else
    echo -e "    ${GRAY}(info mode, no tests run)${RESET}"
fi
echo -e "  Log:     $SESSION_LOG"
echo ""

exit $FAILED
