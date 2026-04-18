#!/usr/bin/env fish
# Bot Mock Test Runner — unified entry point
#
# Usage:
#   fish tests/bot_mock/run_test.fish              # interactive: pick a module
#   fish tests/bot_mock/run_test.fish all           # run all modules
#   fish tests/bot_mock/run_test.fish telegram      # run Telegram tests (alias: tg)
#   fish tests/bot_mock/run_test.fish discord       # run Discord tests (alias: dc)
#   fish tests/bot_mock/run_test.fish list           # list available modules
#   fish tests/bot_mock/run_test.fish cases <mod>   # list test cases in a module

set SCRIPT_DIR (dirname (realpath (status filename)))
set PROJECT_ROOT (realpath $SCRIPT_DIR/../..)
set VENV_PYTHON $SCRIPT_DIR/.venv/bin/python
set BOT_BIN $PROJECT_ROOT/target/debug/octos

# ── Available modules ─────────────────────────────────────────────────────────
# Each module: name  alias  mock_port  test_file  feature  mock_class
set -g MODULES "telegram|tg|5000|test_telegram.py|telegram|mock_tg|MockTelegramServer" \
               "discord|dc|5001|test_discord.py|discord|mock_discord|MockDiscordServer"

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

# ── Module helpers ────────────────────────────────────────────────────────────

function list_modules
    echo ""
    echo -e "$BOLD  Available test modules:$RESET"
    echo ""
    for mod in $MODULES
        set parts (string split "|" $mod)
        set name $parts[1]
        set alias $parts[2]
        set port $parts[3]
        set testf $parts[4]
        echo -e "  $GREEN$name$RESET ($alias)  —  port $port, test file: $testf"
    end
    echo ""
end

function list_cases
    set -l mod $argv[1]
    parse_module $mod

    if not test -f $VENV_PYTHON
        err "Python venv not found. Run a test first to create it."
        return 1
    end

    if not test -f "$SCRIPT_DIR/$MOD_TEST_FILE"
        err "Test file not found: $SCRIPT_DIR/$MOD_TEST_FILE"
        return 1
    end

    echo ""
    echo -e "$BOLD  Test cases in $MOD_NAME ($MOD_TEST_FILE):$RESET"
    echo ""

    set -x PYTHONPATH $SCRIPT_DIR
    # Use --collect-only with quiet mode, then format the output
    set -l cases ($VENV_PYTHON -m pytest $SCRIPT_DIR/$MOD_TEST_FILE --collect-only -q --no-header 2>/dev/null | grep "::" | string replace -r "^.*?::" "")

    set -l current_class ""
    set -l idx 0
    for c in $cases
        # Skip summary lines like "X tests collected"
        string match -q "*test_*" -- $c; or continue
        # Skip warning lines
        string match -q "*Warning*" -- $c; or string match -q "*pytest*" -- $c; and continue

        set -l parts2 (string split "::" $c)
        if test (count $parts2) -eq 2
            set cls $parts2[1]
            set func $parts2[2]
            if test "$cls" != "$current_class"
                set current_class $cls
                echo -e "  $CYAN$cls$RESET"
            end
            set idx (math $idx + 1)
            echo -e "    $GRAY$idx$RESET  $GREEN$func$RESET"
        else if test (count $parts2) -eq 1
            set idx (math $idx + 1)
            echo -e "    $GRAY$idx$RESET  $GREEN$parts2[1]$RESET"
        end
    end

    if test $idx -eq 0
        warn "No test cases found"
    else
        echo ""
        echo -e "  $GRAY$idx test(s) in total$RESET"
    end
    echo ""
end

function resolve_module
    # Returns the module string if arg matches name or alias, empty otherwise
    set -l query $argv[1]
    for mod in $MODULES
        set parts (string split "|" $mod)
        set name $parts[1]
        set alias $parts[2]
        if test "$query" = "$name"; or test "$query" = "$alias"
            echo $mod
            return 0
        end
    end
    return 1
end

function parse_module
    # Parse "name|alias|port|test_file|feature|mock_module|mock_class" into variables
    set -l mod $argv[1]
    set parts (string split "|" $mod)
    set -g MOD_NAME $parts[1]
    set -g MOD_ALIAS $parts[2]
    set -g MOD_PORT $parts[3]
    set -g MOD_TEST_FILE $parts[4]
    set -g MOD_FEATURE $parts[5]
    set -g MOD_MOCK_MODULE $parts[6]
    set -g MOD_MOCK_CLASS $parts[7]
end

# ── Per-module setup functions ────────────────────────────────────────────────

function setup_telegram_env
    # Telegram-specific env checks
    if not set -q TELEGRAM_BOT_TOKEN
        err "TELEGRAM_BOT_TOKEN is not set"
        return 1
    end
    set -g BOT_LOG /tmp/octos_telegram_bot_test.log
    set -g CONFIG_FILE $PROJECT_ROOT/.octos/test_config.json
    set -g EXTRA_ENV_VAR "TELOXIDE_API_URL"
    set -g EXTRA_ENV_VAL "http://127.0.0.1:$MOD_PORT"
    set -g CONFIG_JSON '{
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
}'
    return 0
end

function setup_discord_env
    # Discord: DISCORD_BOT_TOKEN auto-set to dummy if missing
    if not set -q DISCORD_BOT_TOKEN
        set -gx DISCORD_BOT_TOKEN "mock-bot-token-for-testing"
        info "DISCORD_BOT_TOKEN not set, using dummy value (mock mode)"
    end
    set -g BOT_LOG /tmp/octos_discord_bot_test.log
    set -g CONFIG_FILE $PROJECT_ROOT/.octos/test_discord_config.json
    set -g EXTRA_ENV_VAR "DISCORD_API_BASE_URL"
    set -g EXTRA_ENV_VAL "http://127.0.0.1:$MOD_PORT"
    set -g CONFIG_JSON '{
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
}'
    return 0
end

# ── Core runner ───────────────────────────────────────────────────────────────

function run_module
    set -l mod $argv[1]
    parse_module $mod

    section "Running $MOD_NAME tests (port $MOD_PORT)"

    # ── 1. Check ANTHROPIC_API_KEY ───────────────────────────────────────────
    if not set -q ANTHROPIC_API_KEY
        err "ANTHROPIC_API_KEY is not set"
        return 1
    end

    # ── 2. Module-specific env setup ─────────────────────────────────────────
    switch $MOD_NAME
        case telegram
            if not setup_telegram_env
                return 1
            end
        case discord
            setup_discord_env
    end
    ok "Environment variables present"

    # ── 3. Check Python venv ─────────────────────────────────────────────────
    if not test -f $VENV_PYTHON
        info "Creating Python venv..."
        uv venv $SCRIPT_DIR/.venv
        uv pip install fastapi uvicorn httpx pytest pytest-asyncio websockets --python $VENV_PYTHON
    end
    ok "Python venv ready"

    # ── 4. Write test config ─────────────────────────────────────────────────
    section "Writing config"
    mkdir -p (dirname $CONFIG_FILE)
    echo $CONFIG_JSON > $CONFIG_FILE
    ok "Config written to $CONFIG_FILE ($MOD_NAME channel)"

    # ── 5. Kill anything on the mock port ────────────────────────────────────
    section "Preparing mock server"
    set EXISTING_PID (lsof -ti tcp:$MOD_PORT 2>/dev/null)
    if test -n "$EXISTING_PID"
        warn "Port $MOD_PORT in use by PID $EXISTING_PID, killing..."
        kill $EXISTING_PID 2>/dev/null
        for i in (seq 1 10)
            sleep 0.5
            if not lsof -ti tcp:$MOD_PORT >/dev/null 2>&1
                break
            end
        end
    end

    # ── 6. Start mock server ─────────────────────────────────────────────────
    set -l HEALTH_TIMEOUT 3
    if test "$MOD_NAME" = "discord"
        set HEALTH_TIMEOUT 5  # WS needs more time
    end

    set -x PYTHONPATH $SCRIPT_DIR
    $VENV_PYTHON -c "
import time, signal, sys
from $MOD_MOCK_MODULE import $MOD_MOCK_CLASS
server = $MOD_MOCK_CLASS(port=$MOD_PORT)
server.start_background()
print('ready', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
" &
    set MOCK_PID $last_pid

    set -l WAIT_SEC 1
    if test "$MOD_NAME" = "discord"
        set WAIT_SEC 2
    end
    sleep $WAIT_SEC

    if not $VENV_PYTHON -c "
import httpx, sys
try:
    r = httpx.get('http://127.0.0.1:$MOD_PORT/health', timeout=$HEALTH_TIMEOUT)
    sys.exit(0 if r.status_code == 200 else 1)
except Exception as e:
    print(e); sys.exit(1)
" 2>/dev/null
        err "$MOD_NAME Mock server failed to start"
        lsof -i tcp:$MOD_PORT
        return 1
    end
    ok "$MOD_NAME Mock server running on port $MOD_PORT (PID $MOCK_PID)"

    # ── 7. Build bot ─────────────────────────────────────────────────────────
    section "Building octos (--features $MOD_FEATURE)"
    info "This may take a moment on first build..."
    set BUILD_LOG /tmp/octos_$MOD_NAME\_build.log
    cargo build --manifest-path $PROJECT_ROOT/Cargo.toml --bin octos --features $MOD_FEATURE > $BUILD_LOG 2>&1
    if test $status -ne 0
        err "Build failed:"
        cat $BUILD_LOG
        kill $MOCK_PID 2>/dev/null
        return 1
    end
    ok "Build complete ($MOD_FEATURE feature)"

    # ── 8. Start bot ─────────────────────────────────────────────────────────
    section "Starting octos gateway"
    rm -f $BOT_LOG

    # Apply module-specific env vars (global export so child processes inherit)
    set -gx $EXTRA_ENV_VAR $EXTRA_ENV_VAL
    $BOT_BIN gateway --config $CONFIG_FILE > $BOT_LOG 2>&1 &
    set BOT_PID $last_pid
    info "Bot PID: $BOT_PID  |  Log: $BOT_LOG"

    # Poll for ready
    echo ""
    echo -e "$GRAY  Waiting for gateway to start...$RESET"
    set READY 0
    set MAX_WAIT 40
    if test "$MOD_NAME" = "discord"
        set MAX_WAIT 50
    end
    for i in (seq 1 $MAX_WAIT)
        sleep 1
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
        if test "$MOD_NAME" = "discord"; and grep -q "Discord.*bot connected\|Discord channel started" $BOT_LOG 2>/dev/null
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
        return 1
    end
    ok "Gateway ready! ($MOD_NAME channel active)"

    # ── 9. Run tests ─────────────────────────────────────────────────────────
    section "Running $MOD_NAME tests"

    set -x PYTHONPATH $SCRIPT_DIR
    set -x MOCK_BASE_URL http://127.0.0.1:$MOD_PORT

    # Print visual separators to log file
    echo "" >> $BOT_LOG
    echo "╔══════════════════════════════════════════════════════════════╗" >> $BOT_LOG
    echo "║           TEST EXECUTION START: $MOD_NAME" >> $BOT_LOG
    echo "╚══════════════════════════════════════════════════════════════╝" >> $BOT_LOG
    echo "" >> $BOT_LOG

    # Run tests with output synced to log (both stdout and stderr)
    # Merge stderr into stdout, then pipe to tee
    $VENV_PYTHON -m pytest $SCRIPT_DIR/$MOD_TEST_FILE -v --tb=short --no-header ^&1 | tee -a $BOT_LOG
    set -l TEST_EXIT $pipestatus[1]

    # Print test results summary to log
    echo "" >> $BOT_LOG
    echo "╔══════════════════════════════════════════════════════════════╗" >> $BOT_LOG
    if test $TEST_EXIT -eq 0
        echo "║                  ✅ ALL TESTS PASSED                        ║" >> $BOT_LOG
    else
        echo "║                  ❌ SOME TESTS FAILED                       ║" >> $BOT_LOG
    end
    echo "╚══════════════════════════════════════════════════════════════╝" >> $BOT_LOG
    echo "" >> $BOT_LOG

    # ── 10. Cleanup ───────────────────────────────────────────────────────────
    section "Cleanup"
    
    # Kill processes and wait for them to exit
    kill $BOT_PID 2>/dev/null
    kill $MOCK_PID 2>/dev/null
    
    # Wait for processes to terminate (with timeout)
    for i in (seq 1 10)
        if not kill -0 $BOT_PID 2>/dev/null; and not kill -0 $MOCK_PID 2>/dev/null
            break
        end
        sleep 0.5
    end
    
    # Force kill if still running
    kill -9 $BOT_PID 2>/dev/null
    kill -9 $MOCK_PID 2>/dev/null
    
    ok "Processes stopped"

    echo ""
    if test $TEST_EXIT -eq 0
        echo -e "$BOLD$GREEN  🎉 All $MOD_NAME tests passed!$RESET"
    else
        echo -e "$BOLD$RED  💥 Some $MOD_NAME tests failed$RESET"
        echo -e "$GRAY  Bot log: $BOT_LOG$RESET"
    end
    echo ""

    return $TEST_EXIT
end

# ── Help ──────────────────────────────────────────────────────────────────────

function show_help
    echo ""
    echo -e "$BOLD  Bot Mock Test Runner$RESET"
    echo ""
    echo "  Usage:"
    echo "    fish tests/bot_mock/run_test.fish [command] [args]"
    echo ""
    echo "  Commands:"
    echo "    (none)        Interactive mode — pick a module from menu"
    echo "    all           Run all test modules"
    echo "    telegram, tg  Run Telegram tests"
    echo "    discord, dc   Run Discord tests"
    echo "    list          List available test modules"
    echo "    cases <mod>   List test cases in a module (e.g. cases tg)"
    echo "    -h, --help    Show this help message"
    echo ""
    echo "  Examples:"
    echo "    fish tests/bot_mock/run_test.fish              # interactive menu"
    echo "    fish tests/bot_mock/run_test.fish discord      # run Discord tests"
    echo "    fish tests/bot_mock/run_test.fish cases tg     # list Telegram test cases"
    echo ""
end

# ── Main ──────────────────────────────────────────────────────────────────────

set -l ACTION $argv[1]

# Default / help → show help
if test -z "$ACTION"; or test "$ACTION" = "-h"; or test "$ACTION" = "--help"
    show_help
    exit 0
end

switch $ACTION
    case list ls
        list_modules
        exit 0

    case cases
        # fish run_test.fish cases <module>
        set -l target $argv[2]
        if test -z "$target"
            err "Usage: fish run_test.fish cases <module>"
            list_modules
            exit 1
        end
        set -l resolved (resolve_module $target)
        if test -z "$resolved"
            err "Unknown module: $target"
            list_modules
            exit 1
        end
        list_cases $resolved
        exit 0

    case all
        section "Running ALL test modules"
        set -l FAILED 0
        for mod in $MODULES
            if not run_module $mod
                set FAILED 1
            end
        end
        if test $FAILED -eq 0
            echo -e "$BOLD$GREEN  🎉 All modules passed!$RESET"
        else
            echo -e "$BOLD$RED  💥 Some modules failed$RESET"
        end
        exit $FAILED

    case '*'
        set -l resolved (resolve_module $ACTION)
        if test -z "$resolved"
            err "Unknown module: $ACTION"
            list_modules
            exit 1
        end
        run_module $resolved
        exit $status
end
