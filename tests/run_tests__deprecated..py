#!/usr/bin/env python3
"""
Octos Test Runner

Usage:
    tests/run_tests.py <command> [args...]

Commands:
    all                          Run all test suites (bot + cli)
    --test bot [bot-args...]     Run bot mock tests
    --test cli [cli-args...]     Run CLI tests
    -h, --help                   Show this help message
"""

import argparse
import json
import os
import subprocess
import sys
import time
import signal
import re
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Optional

import httpx

SCRIPT_DIR = Path(__file__).parent
PROJECT_ROOT = SCRIPT_DIR.parent

TEST_DIR = Path("/tmp/octos_test")
LOG_DIR = TEST_DIR / "logs"
BINARY_PATH = PROJECT_ROOT / "target" / "release" / "octos"

os.makedirs(LOG_DIR, exist_ok=True)


@dataclass
class Module:
    name: str
    alias: str
    port: int
    test_file: str
    feature: str
    mock_module: str
    mock_class: str


MODULES = [
    Module("telegram", "tg", 5000, "test_telegram.py", "telegram", "mock_tg", "MockTelegramServer"),
    Module("discord", "dc", 5001, "test_discord.py", "discord", "mock_discord", "MockDiscordServer"),
]


def eprint(*args, **kwargs):
    print(*args, **kwargs, file=sys.stderr)


def section(title: str):
    print("")
    print(f"── {title} ")
    print("")


def info(msg: str):
    print(f"  ℹ {msg}")


def ok(msg: str):
    print(f"  ✅ {msg}")


def warn(msg: str):
    print(f"  ⚠️  {msg}")


def err(msg: str):
    print(f"  ❌ {msg}")


def get_session_log() -> Path:
    return LOG_DIR / f"test_{datetime.now().strftime('%Y%m%d_%H%M%S')}.log"


def get_build_log() -> Path:
    return LOG_DIR / "build.log"


def get_module_log(module_name: str) -> Path:
    return LOG_DIR / f"octos_{module_name}_{datetime.now().strftime('%Y%m%d_%H%M%S')}.log"


def check_env() -> bool:
    missing = []
    if not os.environ.get("ANTHROPIC_API_KEY"):
        missing.append("ANTHROPIC_API_KEY")
    if not os.environ.get("TELEGRAM_BOT_TOKEN"):
        missing.append("TELEGRAM_BOT_TOKEN")

    if missing:
        section("Missing required environment variables")
        for var in missing:
            err(f"{var} is not set")
        print("")
        print("  Set them before running, e.g.:")
        print("    export ANTHROPIC_API_KEY=sk-...")
        print("    export TELEGRAM_BOT_TOKEN=123456:ABC...")
        return False
    return True


def build_octos() -> bool:
    section("Building octos (telegram, discord)")
    info(f"Test directory: {TEST_DIR}")
    info(f"Build log: {get_build_log()}")

    cmd = [
        "cargo", "build", "--release", "-p", "octos-cli",
        "--features", "telegram,discord"
    ]

    try:
        result = subprocess.run(
            cmd,
            cwd=PROJECT_ROOT,
            capture_output=True,
            text=True,
        )
        with open(get_build_log(), "w") as f:
            f.write(result.stdout)
            if result.stderr:
                f.write("\n--- STDERR ---\n")
                f.write(result.stderr)

        if result.returncode != 0:
            err("Build failed")
            return False
    except Exception as e:
        err(f"Build failed: {e}")
        return False

    ok("Build complete (telegram, discord)")
    return True


def find_module(query: str) -> Optional[Module]:
    for mod in MODULES:
        if query == mod.name or query == mod.alias:
            return mod
    return None


def list_modules():
    print("")
    print("  Available test modules:")
    print("")
    for mod in MODULES:
        print(f"    {mod.name} ({mod.alias})  —  port {mod.port}, test file: {mod.test_file}")
    print("")


def list_cases(module: Module):
    venv_python = SCRIPT_DIR / "bot_mock_test" / ".venv" / "bin" / "python"
    test_file = SCRIPT_DIR / "bot_mock_test" / module.test_file

    if not venv_python.exists():
        print("Python venv not found. Run a test first to create it.")
        return

    if not test_file.exists():
        print(f"Test file not found: {test_file}")
        return

    print("")
    print(f"  Test cases in {module.name} ({module.test_file}):")
    print("")

    result = subprocess.run(
        [str(venv_python), "-m", "pytest", str(test_file), "--collect-only", "-q", "--no-header"],
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONPATH": str(SCRIPT_DIR / "bot_mock_test")},
    )

    cases = []
    for line in result.stdout.splitlines():
        if "::" in line and "test_" in line:
            parts = line.split("::")
            if len(parts) >= 2:
                cls = parts[-2]
                func = parts[-1]
                cases.append((cls, func))

    idx = 0
    current_class = None
    for cls, func in cases:
        if cls != current_class:
            current_class = cls
            print(f"    {cls}")
        idx += 1
        print(f"      {idx}  {func}")

    if idx == 0:
        print("No test cases found")
    else:
        print("")
        print(f"  {idx} test(s) in total")
    print("")


def setup_venv():
    venv_python = SCRIPT_DIR / "bot_mock_test" / ".venv" / "bin" / "python"
    if not venv_python.exists():
        info("Creating Python venv...")
        subprocess.run(["uv", "venv", str(SCRIPT_DIR / "bot_mock_test" / ".venv")], check=True)
        subprocess.run([
            "uv", "pip", "install",
            "fastapi", "uvicorn", "httpx", "pytest", "pytest-asyncio", "websockets",
            "--python", str(venv_python),
        ], check=True)
    return venv_python


def clear_cache():
    import shutil
    for pattern in ["__pycache__", "*.pyc", ".pytest_cache"]:
        for path in SCRIPT_DIR.glob(f"**/{pattern}"):
            if path.is_dir():
                shutil.rmtree(path, ignore_errors=True)
            elif path.is_file():
                path.unlink(missing_ok=True)

    venv = SCRIPT_DIR / "bot_mock_test" / ".venv"
    if venv.exists():
        for pattern in ["__pycache__", "*.pyc"]:
            for path in venv.glob(f"**/{pattern}"):
                if path.is_dir():
                    shutil.rmtree(path, ignore_errors=True)
                elif path.is_file():
                    path.unlink(missing_ok=True)


def kill_port(port: int):
    try:
        result = subprocess.run(
            ["lsof", "-ti", f"tcp:{port}"],
            capture_output=True,
            text=True,
        )
        pids = result.stdout.strip().splitlines()
        for pid in pids:
            if pid:
                try:
                    os.kill(int(pid), signal.SIGKILL)
                except ProcessLookupError:
                    pass
        if pids:
            time.sleep(1)
    except Exception:
        pass


def wait_for_health(port: int, timeout: int = 5) -> bool:
    start = time.time()
    while time.time() - start < timeout:
        try:
            resp = httpx.get(f"http://127.0.0.1:{port}/health", timeout=2)
            if resp.status_code == 200:
                return True
        except Exception:
            pass
        time.sleep(0.5)
    return False


def run_module(module: Module, test_case: Optional[str] = None) -> bool:
    section(f"Running {module.name} tests (port {module.port})")

    print(f"  octos version: {subprocess.run(
        [str(BINARY_PATH), "--version"],
        capture_output=True,
        text=True,
    ).stdout.strip() or 'unknown'}")

    if not os.environ.get("ANTHROPIC_API_KEY"):
        err("ANTHROPIC_API_KEY is not set")
        return False

    if module.name == "telegram":
        if not os.environ.get("TELEGRAM_BOT_TOKEN"):
            err("TELEGRAM_BOT_TOKEN is not set")
            return False
        extra_env = {"TELOXIDE_API_URL": f"http://127.0.0.1:{module.port}"}
        config = {
            "version": 1,
            "provider": "anthropic",
            "model": "MiniMax-M2.7",
            "api_key_env": "ANTHROPIC_API_KEY",
            "base_url": "https://api.minimaxi.com/anthropic",
            "gateway": {
                "channels": [{"type": "telegram", "settings": {"token_env": "TELEGRAM_BOT_TOKEN"}, "allowed_senders": []}],
                "queue_mode": "interrupt",
            },
        }
    else:
        if not os.environ.get("DISCORD_BOT_TOKEN"):
            os.environ["DISCORD_BOT_TOKEN"] = "mock-bot-token-for-testing"
            info("DISCORD_BOT_TOKEN not set, using dummy value (mock mode)")
        extra_env = {"DISCORD_API_BASE_URL": f"http://127.0.0.1:{module.port}"}
        config = {
            "version": 1,
            "provider": "anthropic",
            "model": "MiniMax-M2.7",
            "api_key_env": "ANTHROPIC_API_KEY",
            "base_url": "https://api.minimaxi.com/anthropic",
            "gateway": {
                "channels": [{"type": "discord", "settings": {"token_env": "DISCORD_BOT_TOKEN"}, "allowed_senders": []}],
                "queue_mode": "collect",
            },
        }

    ok("Environment variables present")

    venv_python = setup_venv()
    ok("Python venv ready")

    config_dir = TEST_DIR / ".octos"
    config_dir.mkdir(parents=True, exist_ok=True)
    config_file = config_dir / f"test_{module.name}_config.json"
    with open(config_file, "w") as f:
        json.dump(config, f, indent=2)
    ok(f"Config written to {config_file}")

    section("Preparing mock server")
    kill_port(module.port)
    clear_cache()

    bot_log = get_module_log(module.name)

    mock_code = f"""
import time, signal, sys, logging
from {module.mock_module} import {module.mock_class}

# Suppress httpx INFO logs to reduce noise
logging.getLogger("httpx").setLevel(logging.WARNING)
logging.getLogger("httpcore").setLevel(logging.WARNING)

server = {module.mock_class}(port={module.port})
server.start_background(log_file='{bot_log}')
print('ready', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
"""

    mock_proc = subprocess.Popen(
        [str(venv_python), "-c", mock_code],
        env={**os.environ, "PYTHONPATH": str(SCRIPT_DIR / "bot_mock_test"), "PYTHONDONTWRITEBYTECODE": "1"},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )

    health_timeout = 5 if module.name == "discord" else 3
    if not wait_for_health(module.port, health_timeout):
        err(f"{module.name} Mock server failed to start")
        mock_proc.terminate()
        return False

    mock_pid = mock_proc.pid
    ok(f"{module.name} Mock server running on port {module.port} (PID {mock_pid})")

    section("Starting octos gateway")
    if not BINARY_PATH.exists():
        err(f"octos binary not found: {BINARY_PATH}")
        mock_proc.terminate()
        return False

    bot_env = {**os.environ, **extra_env}
    bot_proc = subprocess.Popen(
        [str(BINARY_PATH), "gateway", "--config", str(config_file), "--data-dir", str(TEST_DIR)],
        env=bot_env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    bot_pid = bot_proc.pid
    info(f"Bot PID: {bot_pid}")

    print("")
    print("  Waiting for gateway to start...")

    ready = False
    max_wait = 50 if module.name == "discord" else 40
    start = time.time()
    last_log_line = ""

    try:
        while time.time() - start < max_wait:
            if bot_proc.poll() is not None:
                err("Bot process exited unexpectedly")
                break

            # Only show new log lines (avoid spam)
            try:
                with open(bot_log) as f:
                    lines = f.readlines()
                    if lines:
                        current_line = lines[-1].strip()
                        if current_line != last_log_line and any(keyword in current_line.lower() for keyword in ["error", "warn", "ready"]):
                            print(f"  › {current_line}")
                            last_log_line = current_line
            except FileNotFoundError:
                pass

            try:
                with open(bot_log) as f:
                    content = f.read()
                    if re.search(r"gateway.*ready|Gateway ready|\[gateway\] ready", content):
                        ready = True
                        break
            except FileNotFoundError:
                pass

            time.sleep(1)
    finally:
        def cleanup():
            try:
                os.kill(bot_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                os.kill(mock_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            time.sleep(1)
            try:
                os.kill(bot_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                os.kill(mock_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    if not ready:
        err("Bot failed to start. Full log:")
        try:
            with open(bot_log) as f:
                for line in f:
                    print(f"    {line.rstrip()}")
        except FileNotFoundError:
            pass
        cleanup()
        return False

    ok("Gateway ready!")

    section(f"Running {module.name} tests")

    pytest_args = [
        str(venv_python), "-m", "pytest",
        str(SCRIPT_DIR / "bot_mock_test" / module.test_file),
        "-v", "--tb=line", "--no-header", "-p", "no:warnings",
        "--log-cli-level=ERROR",  # Only show ERROR level logs
    ]
    if test_case:
        pytest_args.extend(["-k", test_case])
        info(f"Running specific test: {test_case}")

    result = subprocess.run(
        pytest_args,
        env={**os.environ, "PYTHONPATH": str(SCRIPT_DIR / "bot_mock_test"), "MOCK_BASE_URL": f"http://127.0.0.1:{module.port}"},
    )

    cleanup()

    if result.returncode == 0:
        ok(f"All {module.name} tests passed!")
    else:
        err(f"Some {module.name} tests failed")

    return result.returncode == 0


def run_bot_tests(args) -> bool:
    action = args.bot_action if hasattr(args, 'bot_action') else None
    action2 = args.bot_case if hasattr(args, 'bot_case') else None

    if action in ("-h", "--help"):
        print("")
        print("  Bot Mock Test Runner")
        print("")
        print("  Do NOT run directly. Use:")
        print("    tests/run_tests.py --test bot [args...]")
        print("")
        print("  Arguments:")
        print("    all              Run all bot modules")
        print("    telegram, tg     Run Telegram tests")
        print("    discord, dc      Run Discord tests")
        print("    list             List available modules")
        print("    list <mod>       List test cases in a module")
        print("    <mod> [case]     Run module or specific test case")
        print("")
        print("  Examples:")
        print("    tests/run_tests.py --test bot telegram")
        print("    tests/run_tests.py --test bot list")
        print("    tests/run_tests.py --test bot list tg")
        print("    tests/run_tests.py --test bot telegram test_concurrent_session_creation")
        return True

    if action in ("list", "ls"):
        if action2:
            mod = find_module(action2)
            if not mod:
                print("")
                print(f"Unknown module: {action2}")
                list_modules()
                return False
            list_cases(mod)
        else:
            list_modules()
        return True

    if action == "all":
        section("Running ALL test modules")
        failed = False
        for mod in MODULES:
            if not run_module(mod):
                failed = True
        if not failed:
            print("")
            print("  🎉 All modules passed!")
        else:
            print("")
            print("  💥 Some modules failed")
        return not failed

    mod = find_module(action)
    if mod:
        return run_module(mod, action2)

    if action and action.startswith("test_"):
        print("")
        print(f"Test case '{action}' specified without module name")
        print("")
        print("  Usage: tests/run_tests.py --test bot <module> <test_case>")
        print(f"  Example: tests/run_tests.py --test bot telegram {action}")
        list_modules()
        return False

    if action:
        print("")
        print(f"Unknown module: {action}")
        list_modules()
        return False

    section("Running ALL bot tests")
    failed = False
    for mod in MODULES:
        if not run_module(mod):
            failed = True
    if not failed:
        print("")
        print("  🎉 All modules passed!")
    else:
        print("")
        print("  💥 Some modules failed")
    return not failed


def run_cli_tests(args) -> bool:
    print("")
    print("── Running CLI Tests ")
    print("")

    cli_script = SCRIPT_DIR / "cli_test" / "cli_test.sh"
    if not cli_script.exists():
        print("")
        print(f"CLI test script not found: {cli_script}")
        return False

    cli_args = ["-b", str(BINARY_PATH)]
    if args.verbose:
        cli_args.append("-v")
    if args.output_dir:
        cli_args.extend(["-o", args.output_dir])
    if args.scope:
        cli_args.extend(["-s", args.scope])

    result = subprocess.run(
        ["bash", str(cli_script)] + cli_args,
        env={**os.environ, "OCTOS_TEST_DIR": str(TEST_DIR), "OCTOS_LOG_DIR": str(LOG_DIR)},
    )

    if result.returncode == 0:
        ok("CLI tests passed")
    else:
        err("CLI tests failed")

    return result.returncode == 0


def show_help(test_target: Optional[str] = None):
    print("")
    print("  Octos Test Runner")
    print("")
    print("  Usage:")
    print("    tests/run_tests.py <command> [args...]")
    print("")
    print("  Commands:")
    print("    all                          Run all test suites (bot + cli)")
    print("    --test bot [bot-args...]     Run bot mock tests")
    print("    --test cli [cli-args...]     Run CLI tests")
    print("    -h, --help                   Show this help message")
    print("")

    if test_target is None:
        print("  Bot test arguments (after --test bot):")
        print("    all              Run all bot modules")
        print("    telegram, tg     Run Telegram tests only")
        print("    discord, dc      Run Discord tests only")
        print("    list             List available bot modules")
        print("    list <mod>       List test cases in a module")
        print("    <mod> [case]     Run module or specific test case")
        print("")
        print("  CLI test arguments (after --test cli):")
        print("    -v, --verbose              Verbose output")
        print("    -o, --output-dir DIR       Output directory (default: test-results)")
        print("    -s, --scope SCOPE          Test scope")
        print("    list                       List available test categories")
        print("    list <category>            List test cases in a category")
        print("")
        print("  Examples:")
        print("    tests/run_tests.py all                     # run everything")
        print("    tests/run_tests.py --test bot              # all bot tests")
        print("    tests/run_tests.py --test bot telegram     # Telegram only")
        print("    tests/run_tests.py --test bot list         # list bot modules")
        print("    tests/run_tests.py --test bot list tg      # list Telegram test cases")
        print("    tests/run_tests.py --test bot tg           # run Telegram tests")
        print("    tests/run_tests.py --test cli              # CLI tests")
        print("    tests/run_tests.py --test cli -v           # CLI tests, verbose")
        print("    tests/run_tests.py --test cli list         # List test categories")
    elif test_target == "bot":
        print("  Bot test arguments (after --test bot):")
        print("    all              Run all bot modules")
        print("    telegram, tg     Run Telegram tests only")
        print("    discord, dc      Run Discord tests only")
        print("    list             List available bot modules")
        print("    list <mod>       List test cases in a module")
        print("    <mod> [case]     Run module or specific test case")
        print("")
        print("  Examples:")
        print("    tests/run_tests.py --test bot              # all bot tests")
        print("    tests/run_tests.py --test bot telegram     # Telegram only")
        print("    tests/run_tests.py --test bot list         # list bot modules")
        print("    tests/run_tests.py --test bot list tg      # list Telegram test cases")
        print("    tests/run_tests.py --test bot tg           # run Telegram tests")
    elif test_target == "cli":
        print("  CLI test arguments (after --test cli):")
        print("    -v, --verbose              Verbose output")
        print("    -o, --output-dir DIR       Output directory (default: test-results)")
        print("    -s, --scope SCOPE          Test scope")
        print("    list                       List available test categories")
        print("    list <category>            List test cases in a category")
        print("")
        print("  Examples:")
        print("    tests/run_tests.py --test cli              # CLI tests")
        print("    tests/run_tests.py --test cli -v           # CLI tests, verbose")
        print("    tests/run_tests.py --test cli list         # List test categories")
        print("    tests/run_tests.py --test cli -s Init      # Run Init tests")

    print("")
    print("  Environment:")
    print("    ANTHROPIC_API_KEY    Required for bot LLM tests")
    print("    TELEGRAM_BOT_TOKEN   Required for Telegram bot tests")
    print("    DISCORD_BOT_TOKEN   Optional (auto-set for mock mode)")
    print("")
    print(f"  Test directory: {TEST_DIR}")
    print(f"  Logs: {LOG_DIR}")


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Octos Test Runner",
        add_help=False,
        usage="%(prog)s <command> [args...]"
    )

    parser.add_argument("-h", "--help", dest="show_help", action="store_true")
    parser.add_argument("--test", dest="test_target", nargs="?", const="bot", default=None)
    parser.add_argument("test_args", nargs="*")

    return parser


def main():
    parser = create_parser()
    args = parser.parse_args()

    if args.show_help:
        show_help()
        return 0

    if args.test_target is None and not args.test_args:
        show_help()
        return 0

    if args.test_args and args.test_args[0] == "all":
        if not check_env():
            return 1
        if not build_octos():
            return 1

        section("Running ALL test suites")

        bot_args = argparse.Namespace()
        bot_args.bot_action = "all"
        bot_args.bot_case = None

        cli_args = argparse.Namespace()
        cli_args.verbose = False
        cli_args.output_dir = None
        cli_args.scope = None
        cli_args.cli_action = None

        failed = False
        if not run_bot_tests(bot_args):
            failed = True
        if not run_cli_tests(cli_args):
            failed = True

        section("Test Summary")
        print(f"  Date:    {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
        print(f"  Result:  {'PASSED' if not failed else 'FAILED'}")
        print(f"  Logs:    {LOG_DIR}")
        print("")
        if not failed:
            print("  🎉 All tests passed!")
        else:
            print("  💥 Some tests failed")
        return 1 if failed else 0

    if args.test_target == "bot":
        if not args.test_args:
            show_help("bot")
            return 0

        action = args.test_args[0]
        action2 = args.test_args[1] if len(args.test_args) > 1 else None

        if action in ("-h", "--help"):
            show_help("bot")
            return 0

        if action in ("list", "ls"):
            if action2:
                mod = find_module(action2)
                if not mod:
                    err(f"Unknown module: {action2}")
                    list_modules()
                    return 1
                list_cases(mod)
            else:
                list_modules()
            return 0

        if action == "all":
            if not check_env():
                return 1
            if not build_octos():
                return 1
            bot_args = argparse.Namespace()
            bot_args.bot_action = "all"
            bot_args.bot_case = None
            return 0 if run_bot_tests(bot_args) else 1

        mod = find_module(action)
        if mod:
            if not check_env():
                return 1
            if not build_octos():
                return 1
            bot_args = argparse.Namespace()
            bot_args.bot_action = action
            bot_args.bot_case = action2
            return 0 if run_bot_tests(bot_args) else 1

        err(f"Unknown bot action: {action}")
        show_help("bot")
        return 1

    if args.test_target == "cli":
        if not check_env():
            return 1
        if not build_octos():
            return 1

        cli_args = argparse.Namespace()
        cli_args.verbose = "-v" in args.test_args or "--verbose" in args.test_args
        cli_args.output_dir = None
        cli_args.scope = None

        filtered_args = [a for a in args.test_args if not a.startswith("-")]
        cli_args.cli_action = filtered_args[0] if filtered_args else None

        return 0 if run_cli_tests(cli_args) else 1

    err(f"Unknown command")
    show_help()
    return 1


if __name__ == "__main__":
    sys.exit(main())
