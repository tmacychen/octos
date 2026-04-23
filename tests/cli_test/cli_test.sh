#!/usr/bin/env bash
# Octos CLI Automated Test Script — invoked by run_tests.sh
#
# Do NOT run this script directly. Use:
#   tests/run_tests.sh --test cli [args...]
#
# CLI test arguments:
#   -v, --verbose       Verbose output
#   -o, --output-dir    Output directory (default: test-results)
#   -c, --config        Test config file (default: cli_test/test_cases.json)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="${SCRIPT_DIR}/test_cases.json"

# ── Must be invoked from run_tests.sh ────────────────────────────────────────
if [[ -z "${OCTOS_TEST_DIR:-}" ]]; then
    echo ""
    echo -e "\033[0;31m  ❌ This script cannot be run directly.\033[0m"
    echo ""
    echo "  Please use the unified test runner:"
    echo ""
    echo "    tests/run_tests.sh --test cli [args...]"
    echo ""
    echo "  Available args: -v | -o <dir> | -c <file>"
    echo ""
    exit 1
fi

# Default values
OCTOS_BINARY="octos"
OUTPUT_DIR="test-results"
VERBOSE=false
CANCELLED=false
TEST_SCOPE="all"  # all or specific category

# Unified test runner presets
TEST_DIR="${OCTOS_TEST_DIR:-/tmp/octos_test}"
LOG_DIR="${OCTOS_LOG_DIR:-$TEST_DIR/logs}"

# Counters
TOTAL=0
PASSED=0
FAILED=0

# Arrays for results
declare -a RESULTS=()

# Timestamp
TEST_DATE=$(date '+%Y-%m-%d %H:%M:%S')
REPORT_DATE=$(date '+%Y-%m-%d_%H%M')
TIMESTAMP=$(date '+%Y%m%d_%H%M%S')
LOG_FILE=""
CURRENT_CATEGORY=""
CATEGORY_TEST_DIR=""  # Current category test directory

# Colors (if terminal supports)
if [[ -t 1 ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    GRAY='\033[0;90m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    CYAN=''
    GRAY=''
    BOLD=''
    NC=''
fi

usage() {
    cat << EOF
Do NOT run directly. Use:
  tests/run_tests.sh --test cli [args...]

Arguments:
    -v, --verbose           Verbose output
    -o, --output-dir DIR    Output directory (default: test-results)
    -s, --scope SCOPE       Test scope: all|CLI|Init|Clean|Status|Completions|Skills|Auth|Channels|Cron|Chat|Gateway|Serve|Docs
    list                    List available test categories and exit
EOF
}

log() {
    local msg="$1"
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[${timestamp}] $msg" >> "$LOG_FILE"
}

verbose_log() {
    local msg="$1"
    log "$msg"
    if [[ "$VERBOSE" == true ]]; then
        echo -e "${GRAY}[$msg]${NC}"
    fi
}

cancel_handler() {
    CANCELLED=true
    echo -e "\n${YELLOW}[CANCELLED] Test run cancelled by user${NC}"
    log "[CANCELLED] Test run cancelled by user"

    # Kill any running processes
    pkill -f "octos" 2>/dev/null || true

    # Generate partial report
    if [[ ${#RESULTS[@]} -gt 0 ]]; then
        local partial_report="${OUTPUT_DIR}/CLI_TEST_REPORT_CANCELLED_$(date '+%Y%m%d_%H%M%S').md"
        {
            echo "# Octos CLI Test - Cancelled"
            echo ""
            echo "Test run was cancelled. Partial results:"
            echo ""
            echo "| ID | Category | Test Name | Status |"
            echo "|----|----------|-----------|--------|"
            for r in "${RESULTS[@]}"; do
                echo "$r"
            done
        } >> "$partial_report"
        echo -e "${YELLOW}Partial report: $partial_report${NC}"
    fi

    exit 1
}

trap cancel_handler SIGINT SIGTERM

parse_json() {
    local json="$1"
    local key="$2"
    echo "$json" | grep -o "\"${key}\":[^,}]*" | sed 's/.*: *"\?\([^"]*\)"\?.*/\1/' | tr -d '"'
}

run_cli_test() {
    local test_id="$1"
    local category="$2"
    local test_name="$3"
    local cmd_args="$4"
    local expected="$5"
    local validation="${6:-contains}"
    local timeout="${7:-60}"

    TOTAL=$((TOTAL + 1))

    verbose_log "[EXEC] octos $cmd_args"
    log "[TEST_DIR] $CATEGORY_TEST_DIR"

    local stdout stderr exit_code
    if "$OCTOS_BINARY" $cmd_args > "$TEMP_DIR/octos_stdout_$$.txt" 2> "$TEMP_DIR/octos_stderr_$$.txt"; then
        exit_code=0
    else
        exit_code=$?
    fi

    stdout=$(cat "$TEMP_DIR/octos_stdout_$$.txt" 2>/dev/null || echo "")
    stderr=$(cat "$TEMP_DIR/octos_stderr_$$.txt" 2>/dev/null || echo "")
    rm -f "$TEMP_DIR/octos_stdout_$$.txt" "$TEMP_DIR/octos_stderr_$$.txt"

    local actual="$stdout$stderr"
    local passed=false

    case "$validation" in
        contains)
            if echo "$actual" | grep -q -F -- "$expected"; then
                passed=true
            fi
            ;;
        not_contains)
            if ! echo "$actual" | grep -q -F -- "$expected"; then
                passed=true
            fi
            ;;
        exitcode)
            if [[ "$exit_code" -eq "$expected" ]]; then
                passed=true
            fi
            ;;
    esac

    if [[ "$passed" == true ]]; then
        PASSED=$((PASSED + 1))
        status="PASS"
        color="$GREEN"
    else
        FAILED=$((FAILED + 1))
        status="FAIL"
        color="$RED"
    fi

    # Truncate actual for summary
    local actual_truncated="${actual:0:200}"
    actual_truncated="${actual_truncated//$'\n'/ }"
    actual_truncated="${actual_truncated//$'\r'/ }"

    RESULTS+=("| $test_id | $category | $test_name | $status |")

    # Always log to file
    log "[EXEC] octos $cmd_args"
    log "[EXITCODE] $exit_code"
    log "[STDOUT] $stdout"
    [[ -n "$stderr" ]] && log "[STDERR] $stderr"
    log "[STATUS] $status"
    log ""

    # Verbose output
    if [[ "$VERBOSE" == true ]]; then
        echo -e "${CYAN}[EXEC] octos $cmd_args${NC}"
        echo -e "${GRAY}[EXITCODE] $exit_code${NC}"
        if [[ -n "$stdout" ]]; then
            echo -e "${GRAY}[STDOUT]${NC}"
            echo -e "${GRAY}$stdout${NC}"
        fi
        if [[ -n "$stderr" ]]; then
            echo -e "${YELLOW}[STDERR]${NC}"
            echo -e "${YELLOW}$stderr${NC}"
        fi
        echo ""
    fi

    echo -e "$color[$status]${NC} $test_id $test_name"
}

run_file_check() {
    local test_id="$1"
    local category="$2"
    local test_name="$3"
    local path="$4"
    local should_exist="${5:-true}"

    TOTAL=$((TOTAL + 1))

    log "[FILE CHECK] Test directory: $CATEGORY_TEST_DIR"
    log "[FILE CHECK] Checking path: $path"

    # Small delay to ensure previous command has completed file operations
    sleep 0.1

    local exists=false
    local retry_count=0
    local max_retries=5
    
    # Retry logic for file existence check
    while [[ $retry_count -lt $max_retries ]]; do
        if [[ -e "$path" ]]; then
            exists=true
            log "[FILE CHECK] File exists: YES (attempt $((retry_count + 1)))"
            break
        else
            retry_count=$((retry_count + 1))
            if [[ $retry_count -lt $max_retries ]]; then
                log "[FILE CHECK] File not found, retrying ($retry_count/$max_retries)..."
                sleep 0.2
            fi
        fi
    done
    
    if [[ "$exists" == false ]]; then
        log "[FILE CHECK] File exists: NO (after $max_retries attempts)"
        # Debug: list directory contents if parent exists
        local parent_dir
        parent_dir=$(dirname "$path")
        if [[ -d "$parent_dir" ]]; then
            log "[FILE CHECK] Parent directory exists: $parent_dir"
            log "[FILE CHECK] Parent directory contents:"
            ls -la "$parent_dir" >> "$LOG_FILE" 2>&1
        else
            log "[FILE CHECK] Parent directory does NOT exist: $parent_dir"
        fi
    fi

    local passed=false
    if [[ "$exists" == "$should_exist" ]]; then
        passed=true
    fi

    if [[ "$passed" == true ]]; then
        PASSED=$((PASSED + 1))
        status="PASS"
        color="$GREEN"
    else
        FAILED=$((FAILED + 1))
        status="FAIL"
        color="$RED"
    fi

    local actual_msg
    if [[ "$exists" == true ]]; then
        actual_msg="Path exists: $path"
    else
        actual_msg="Path not found: $path"
    fi

    RESULTS+=("| $test_id | $category | $test_name | $status |")

    log "[FILE CHECK] $path"
    log "[STATUS] $status"
    log ""

    if [[ "$VERBOSE" == true ]]; then
        echo -e "${CYAN}[FILE CHECK] $path${NC}"
        echo -e "${GRAY}[STATUS] $status${NC}"
        echo ""
    fi

    echo -e "$color[$status]${NC} $test_id $test_name"
}

check_jq() {
    if ! command -v jq &> /dev/null; then
        echo -e "${RED}[ERROR] jq is required for JSON parsing${NC}"
        echo -e "${YELLOW}Install jq:${NC}"
        echo -e "  macOS: brew install jq"
        echo -e "  Ubuntu/Debian: apt install jq"
        exit 1
    fi
}

list_categories() {
    check_jq
    
    if [[ ! -f "$CONFIG_FILE" ]]; then
        echo -e "${RED}[ERROR] Config file not found: $CONFIG_FILE${NC}"
        exit 1
    fi

    echo -e "${CYAN}Available Test Categories:${NC}"
    echo -e "${GRAY}========================================${NC}"
    
    # Extract unique categories and count tests per category
    jq -r '.tests[].category' "$CONFIG_FILE" | sort | uniq -c | sort -rn | while read count category; do
        printf "  ${GREEN}%-20s${NC} %d tests\n" "$category" "$count"
    done
    
    echo -e "${GRAY}========================================${NC}"
    local total
    total=$(jq '.tests | length' "$CONFIG_FILE")
    echo -e "Total: ${BOLD}$total${NC} test cases"
    echo ""
    echo -e "${YELLOW}Usage examples:${NC}"
    echo -e "  tests/run_tests.sh --test cli list             # List all categories"
    echo -e "  tests/run_tests.sh --test cli list Init        # List Init test cases"
    echo -e "  tests/run_tests.sh --test cli -s Init          # Run only Init tests"
    echo -e "  tests/run_tests.sh --test cli                  # Run all tests"
}

list_category_cases() {
    local target_category="$1"
    check_jq
    
    if [[ ! -f "$CONFIG_FILE" ]]; then
        echo -e "${RED}[ERROR] Config file not found: $CONFIG_FILE${NC}"
        exit 1
    fi

    # Check if category exists
    local category_exists
    category_exists=$(jq -r --arg cat "$target_category" '.tests[] | select(.category == $cat) | .category' "$CONFIG_FILE" | head -1)
    
    if [[ -z "$category_exists" ]]; then
        echo -e "${RED}[ERROR] Unknown category: $target_category${NC}"
        echo ""
        list_categories
        exit 1
    fi

    echo -e "${CYAN}Test cases in ${GREEN}${target_category}${NC}:${NC}"
    echo -e "${GRAY}========================================${NC}"
    echo ""

    local idx=0
    local total
    total=$(jq --arg cat "$target_category" '[.tests[] | select(.category == $cat)] | length' "$CONFIG_FILE")
    
    jq -r --arg cat "$target_category" '.tests[] | select(.category == $cat) | "\(.id)|\(.name)|\(.command)|\(.timeout // 60)"' "$CONFIG_FILE" | while IFS='|' read -r id name cmd timeout; do
        idx=$((idx + 1))
        echo -e "  ${GRAY}${idx}${NC}  ${GREEN}${name}${NC}"
        echo -e "     ${GRAY}ID: $id | Command: $cmd | Timeout: ${timeout}s${NC}"
        echo ""
    done

    echo -e "${GRAY}========================================${NC}"
    echo -e "Total: ${BOLD}${total}${NC} test case(s) in ${GREEN}${target_category}${NC}"
    echo ""
    echo -e "${YELLOW}Usage:${NC}"
    echo -e "  tests/run_tests.sh --test cli -s ${target_category}   # Run ${target_category} tests"
}

load_tests_from_json() {
    if [[ ! -f "$CONFIG_FILE" ]]; then
        echo -e "${RED}[ERROR] Config file not found: $CONFIG_FILE${NC}"
        exit 1
    fi

    check_jq

    echo -e "${CYAN}Loading tests from: $CONFIG_FILE${NC}"
    log "Loading test configuration from: $CONFIG_FILE"

    TEST_COUNT=$(jq '.tests | length' "$CONFIG_FILE")
    echo -e "${CYAN}Found $TEST_COUNT test cases${NC}"
    log "Found $TEST_COUNT test cases"
}

run_tests_from_json() {
    check_jq

    TEST_COUNT=$(jq '.tests | length' "$CONFIG_FILE")
    local skipped=0

    for i in $(seq 0 $((TEST_COUNT - 1))); do
        if [[ "$CANCELLED" == true ]]; then
            break
        fi

        TEST_ID=$(jq -r ".tests[$i].id" "$CONFIG_FILE")
        CATEGORY=$(jq -r ".tests[$i].category" "$CONFIG_FILE")
        NAME=$(jq -r ".tests[$i].name" "$CONFIG_FILE")
        COMMAND=$(jq -r ".tests[$i].command" "$CONFIG_FILE")
        EXPECTED=$(jq -r ".tests[$i].expected" "$CONFIG_FILE")
        VALIDATION=$(jq -r ".tests[$i].validation // \"contains\"" "$CONFIG_FILE")
        TIMEOUT=$(jq -r ".tests[$i].timeout // 60" "$CONFIG_FILE")
        TEST_TYPE=$(jq -r ".tests[$i].type // \"cli\"" "$CONFIG_FILE")
        FILE_PATH=$(jq -r ".tests[$i].path // \"\"" "$CONFIG_FILE")
        SHOULD_EXIST=$(jq -r ".tests[$i].should_exist // true" "$CONFIG_FILE")

        # Filter by test scope
        if [[ "$TEST_SCOPE" != "all" ]] && [[ "$CATEGORY" != "$TEST_SCOPE" ]]; then
            skipped=$((skipped + 1))
            continue
        fi

        # Create isolated test directory per category with timestamp
        if [[ "$CATEGORY" != "$CURRENT_CATEGORY" ]]; then
            # Cleanup previous category test directory
            if [[ -n "$CATEGORY_TEST_DIR" ]] && [[ -d "$CATEGORY_TEST_DIR" ]]; then
                log "[CLEANUP] Removing previous test directory: $CATEGORY_TEST_DIR"
                rm -rf "$CATEGORY_TEST_DIR"
                verbose_log "[CLEANUP] Removed: $CATEGORY_TEST_DIR"
            fi

            CURRENT_CATEGORY="$CATEGORY"
            CATEGORY_TEST_DIR="${TEST_DIR}/${CATEGORY}_${TIMESTAMP}"
            mkdir -p "$CATEGORY_TEST_DIR"
            log "[SETUP] Created isolated test directory for $CATEGORY: $CATEGORY_TEST_DIR"
            verbose_log "[SETUP] Test directory: $CATEGORY_TEST_DIR"

            echo -e "\n${YELLOW}[$CATEGORY]${NC}"
            log "[SECTION] $CATEGORY"
        fi

        # Replace variables with category-specific test directory
        COMMAND=$(echo "$COMMAND" | sed "s|{testDir}|$CATEGORY_TEST_DIR|g" | sed "s|{tempDir}|$TEMP_DIR|g")

        if [[ "$TEST_TYPE" == "file_check" ]]; then
            CHECK_PATH=$(echo "$FILE_PATH" | sed "s|{testDir}|$CATEGORY_TEST_DIR|g" | sed "s|{tempDir}|$TEMP_DIR|g")
            run_file_check "$TEST_ID" "$CATEGORY" "$NAME" "$CHECK_PATH" "$SHOULD_EXIST"
        else
            run_cli_test "$TEST_ID" "$CATEGORY" "$NAME" "$COMMAND" "$EXPECTED" "$VALIDATION" "$TIMEOUT"
        fi
    done

    # Final cleanup for last category
    if [[ -n "$CATEGORY_TEST_DIR" ]] && [[ -d "$CATEGORY_TEST_DIR" ]]; then
        log "[CLEANUP] Removing final test directory: $CATEGORY_TEST_DIR"
        verbose_log "[CLEANUP] Removed: $CATEGORY_TEST_DIR"
        rm -rf "$CATEGORY_TEST_DIR"
    fi

    if [[ $skipped -gt 0 ]]; then
        echo -e "\n${GRAY}Skipped $skipped tests (scope: $TEST_SCOPE)${NC}"
        log "Skipped $skipped tests (scope: $TEST_SCOPE)"
    fi
}

main() {
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -b|--binary)
                OCTOS_BINARY="$2"
                shift 2
                ;;
            -o|--output-dir)
                if [[ -z "${2:-}" ]]; then
                    echo -e "${RED}[ERROR] Option $1 requires a value${NC}"
                    usage
                    exit 1
                fi
                OUTPUT_DIR="$2"
                shift 2
                ;;
            -v|--verbose)
                VERBOSE=true
                shift
                ;;
            -s|--scope)
                if [[ -z "${2:-}" ]]; then
                    echo -e "${RED}[ERROR] Option $1 requires a value${NC}"
                    usage
                    exit 1
                fi
                TEST_SCOPE="$2"
                shift 2
                ;;
            list)
                local list_target="${2:-}"
                if [[ -n "$list_target" ]]; then
                    list_category_cases "$list_target"
                else
                    list_categories
                fi
                exit 0
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                echo -e "${RED}[ERROR] Unknown option: $1${NC}"
                usage
                exit 1
                ;;
        esac
    done

    # Setup directories — use unified test directory
    mkdir -p "$OUTPUT_DIR"
    TEMP_DIR="${TEST_DIR}/temp"
    mkdir -p "$TEMP_DIR"

    LOG_FILE="$LOG_DIR/cli_test.log"

    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}Octos CLI Automated Test${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo -e "${GRAY}Test Time: $TEST_DATE${NC}"
    echo -e "${GRAY}Binary: $OCTOS_BINARY${NC}"
    echo -e "${GRAY}Log File: $LOG_FILE${NC}"
    echo -e "${GRAY}Base Test Directory: $TEST_DIR${NC}"
    echo ""

    log "========================================"
    log "Octos CLI Automated Test"
    log "========================================"
    log "Test Time: $TEST_DATE"
    log "Binary: $OCTOS_BINARY"
    log "Verbose Mode: $VERBOSE"
    log "Base Test Directory: $TEST_DIR"
    log "Isolation Mode: Per-category with timestamp"
    log ""

    # Check if binary exists
    if [[ ! -x "$OCTOS_BINARY" ]]; then
        if ! command -v "$OCTOS_BINARY" &> /dev/null; then
            echo -e "${RED}[ERROR] Binary not found: $OCTOS_BINARY${NC}"
            echo -e "${YELLOW}Please run: tests/run_tests.sh --test cli${NC}"
            log "[ERROR] Binary not found: $OCTOS_BINARY"
            exit 1
        fi
    fi

    echo -e "${GRAY}Test workspace: $TEST_DIR${NC}"
    log "Test workspace: $TEST_DIR"
    echo ""

    CURRENT_CATEGORY=""
    load_tests_from_json
    run_tests_from_json

    # ========================================
    # Generate Brief Report
    # ========================================
    echo -e "\n${CYAN}========================================${NC}"
    echo -e "${CYAN}Generating Brief Report...${NC}"
    log "========================================"
    log "Generating Brief Report..."

    local report_path="${OUTPUT_DIR}/CLI_TEST_REPORT_${REPORT_DATE}.md"
    local pass_rate=0
    if [[ $TOTAL -gt 0 ]]; then
        pass_rate=$(( PASSED * 100 / TOTAL ))
    fi

    # Write brief report to file
    {
        echo "# Octos CLI Test Report"
        echo ""
        echo "## Summary"
        echo ""
        echo "- **Test Date**: $TEST_DATE"
        echo "- **Scope**: $TEST_SCOPE"
        echo "- **Total**: $TOTAL"
        echo "- **Passed**: $PASSED"
        echo "- **Failed**: $FAILED"
        echo "- **Pass Rate**: ${pass_rate}%"
        echo ""
        echo "## Failed Tests"
        echo ""
        if [[ $FAILED -eq 0 ]]; then
            echo "✅ All tests passed!"
        else
            echo "| ID | Category | Test Name |"
            echo "|----|----------|-----------|"
            for r in "${RESULTS[@]}"; do
                if echo "$r" | grep -q "FAIL"; then
                    # Extract test info from result line
                    echo "$r" | sed 's/| \([^ ]*\) | \([^ ]*\) | \(.*\) | FAIL |/| \1 | \2 | \3 |/'
                fi
            done
        fi
        echo ""
        echo "---"
        echo "*Generated at $TEST_DATE*"
    } > "$report_path"

    # Print summary to stdout
    echo -e "\n${CYAN}========================================${NC}"
    echo -e "${BOLD}Test Summary${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo -e "  Scope:     $TEST_SCOPE"
    echo -e "  Total:     $TOTAL"
    echo -e "  Passed:    ${GREEN}$PASSED${NC}"
    echo -e "  Failed:    ${RED}$FAILED${NC}"
    echo -e "  Pass Rate: ${pass_rate}%"
    echo -e ""
    echo -e "  Report:    ${GREEN}$report_path${NC}"
    echo -e "  Log:       ${GRAY}$LOG_FILE${NC}"
    echo -e "${CYAN}========================================${NC}"

    log "Report saved to: $report_path"
    log "Log saved to: $LOG_FILE"
    log "========================================"
    log "SUMMARY: Total=$TOTAL Passed=$PASSED Failed=$FAILED PassRate=${pass_rate}%"

    # Remove cancel handler
    trap - SIGINT SIGTERM

    if [[ $FAILED -gt 0 ]]; then
        exit 1
    else
        exit 0
    fi
}

main "$@"
