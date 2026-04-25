#!/usr/bin/env python3
"""
Octos Test Runner - Unified test execution tool.

Usage:
    tests/test_run.py <command> [args...]

Commands:
    all                          Run all test suites (bot + cli)
    --test bot [bot-args...]     Run bot mock tests
    --test cli [cli-args...]     Run CLI tests
    -h, --help                   Show this help message

Bot test arguments (after --test bot):
    all              Run all tests
    telegram, tg     Run Telegram tests only
    discord, dc      Run Discord tests only
    list             List available bot modules
    list <mod>       List test cases in a module
    <mod> [case]     Run module or specific test case

CLI test arguments (after --test cli):
    -v, --verbose              Verbose output
    -o, --output-dir DIR       Output directory (default: test-results)
    -s, --scope SCOPE          Test scope
    list                       List available test categories
    list <category>            List test cases in a category

Examples:
    tests/test_run.py all                     # run everything
    tests/test_run.py --test bot              # all bot tests
    tests/test_run.py --test bot telegram     # Telegram only
    tests/test_run.py --test bot list         # list bot modules
    tests/test_run.py --test bot tg list      # list Telegram test cases
    tests/test_run.py --test bot tg           # run Telegram tests
    tests/test_run.py --test bot tg test_new_default  # run specific test
    tests/test_run.py --test cli              # CLI tests
    tests/test_run.py --test cli -v           # CLI tests, verbose
    tests/test_run.py --test cli list         # List test categories

Environment:
    ANTHROPIC_API_KEY    Required for bot LLM tests
    TELEGRAM_BOT_TOKEN   Required for Telegram bot tests
    DISCORD_BOT_TOKEN    Optional (auto-set for mock mode)

Test directory: /tmp/octos_test
Logs: /tmp/octos_test/logs
"""

import argparse
import hashlib
import json
import logging
import os
import re
import signal
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import httpx

# Import CLI test module
from cli_test.test_cli import run_cli_tests as run_cli_tests_module

# Constants
SCRIPT_DIR = Path(__file__).parent
PROJECT_ROOT = SCRIPT_DIR.parent
TEST_DIR = Path("/tmp/octos_test")
LOG_DIR = TEST_DIR / "logs"
BINARY_PATH = PROJECT_ROOT / "target" / "release" / "octos"
BOT_TEST_DIR = SCRIPT_DIR / "bot_mock_test"
CLI_TEST_DIR = SCRIPT_DIR / "cli_test"

# Ensure directories exist
os.makedirs(LOG_DIR, exist_ok=True)


class LoggerManager:
    """Manage multiple loggers for different modules."""
    
    def __init__(self):
        self.loggers: Dict[str, logging.Logger] = {}
        timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
        # Main test runner log
        self.main_logger = self._create_logger("main", LOG_DIR / f"01_runner_{timestamp}.log")
    
    def _create_logger(self, name: str, log_file: Optional[Path] = None) -> logging.Logger:
        """Create a logger with both stdout and file handlers."""
        logger = logging.getLogger(f"octos.{name}")
        logger.setLevel(logging.INFO)
        logger.handlers.clear()
        
        # Format
        formatter = logging.Formatter(
            f"%(asctime)s [{name.upper()}] %(levelname)s %(message)s",
            datefmt="%Y-%m-%d %H:%M:%S"
        )
        
        # Stdout handler
        stdout_handler = logging.StreamHandler(sys.stdout)
        stdout_handler.setFormatter(formatter)
        logger.addHandler(stdout_handler)
        
        # File handler
        if log_file:
            log_file.parent.mkdir(parents=True, exist_ok=True)
            file_handler = logging.FileHandler(log_file)
            file_handler.setFormatter(formatter)
            logger.addHandler(file_handler)
        
        return logger
    
    def get_module_logger(self, module_name: str) -> logging.Logger:
        """Get or create a logger for a specific module. All logs go to main log file."""
        if module_name not in self.loggers:
            # Module logger: reuse main logger's file handler
            logger = logging.getLogger(f"octos.{module_name}")
            logger.setLevel(logging.INFO)
            logger.handlers.clear()
            
            # Format (same as main logger)
            formatter = logging.Formatter(
                f"%(asctime)s [{module_name.upper()}] %(levelname)s %(message)s",
                datefmt="%Y-%m-%d %H:%M:%S"
            )
            
            # Stdout handler
            stdout_handler = logging.StreamHandler(sys.stdout)
            stdout_handler.setFormatter(formatter)
            logger.addHandler(stdout_handler)
            
            # Reuse main logger's file handler
            for handler in self.main_logger.handlers:
                if isinstance(handler, logging.FileHandler):
                    # Create a new FileHandler pointing to the same file
                    file_handler = logging.FileHandler(handler.baseFilename)
                    file_handler.setFormatter(formatter)
                    logger.addHandler(file_handler)
                    break
            
            self.loggers[module_name] = logger
        return self.loggers[module_name]
    
    def info(self, msg: str):
        self.main_logger.info(msg)
    
    def error(self, msg: str):
        self.main_logger.error(msg)
    
    def warning(self, msg: str):
        self.main_logger.warning(msg)


logger_mgr = LoggerManager()
log = logger_mgr.main_logger


def print_help():
    """Print help message."""
    print("""
  Octos Test Runner

  Usage:
    tests/test_run.py <command> [args...]

  Commands:
    all                          Run all test suites (bot + cli)
    --test bot [bot-args...]     Run bot mock tests
    --test cli [cli-args...]     Run CLI tests
    -h, --help                   Show this help message

  Bot test arguments (after --test bot):
    all              Run all tests
    telegram, tg     Run Telegram tests only
    discord, dc      Run Discord tests only
    list             List available bot modules
    list <mod>       List test cases in a module
    <mod> [case]     Run module or specific test case

  CLI test arguments (after --test cli):
    -v, --verbose              Verbose output
    -o, --output-dir DIR       Output directory (default: test-results)
    -s, --scope SCOPE          Test scope
    list                       List available test categories
    list <category>            List test cases in a category

  Examples:
    tests/test_run.py all                     # run everything
    tests/test_run.py --test bot              # all bot tests
    tests/test_run.py --test bot telegram     # Telegram only
    tests/test_run.py --test bot list         # list bot modules
    tests/test_run.py --test bot tg list      # list Telegram test cases
    tests/test_run.py --test bot tg           # run Telegram tests
    tests/test_run.py --test bot tg test_new_default  # run specific test
    tests/test_run.py --test cli              # CLI tests
    tests/test_run.py --test cli -v           # CLI tests, verbose
    tests/test_run.py --test cli list         # List test categories

  Environment:
    ANTHROPIC_API_KEY    Required for bot LLM tests
    TELEGRAM_BOT_TOKEN   Required for Telegram bot tests
    DISCORD_BOT_TOKEN    Optional (auto-set for mock mode)

  Test directory: /tmp/octos_test
  Logs: /tmp/octos_test/logs
""")


def check_environment(required_vars: List[str]) -> bool:
    """Check if required environment variables are set."""
    missing = [var for var in required_vars if not os.environ.get(var)]
    if missing:
        log.error("Missing required environment variables:")
        for var in missing:
            log.error(f"  - {var}")
        return False
    return True


def compute_file_hash(file_path: Path) -> str:
    """Compute MD5 hash of a file."""
    hasher = hashlib.md5()
    with open(file_path, 'rb') as f:
        for chunk in iter(lambda: f.read(8192), b''):
            hasher.update(chunk)
    return hasher.hexdigest()


def compute_directory_hash(directory: Path) -> Dict[str, str]:
    """Compute hashes for all files in a directory."""
    hashes = {}
    if directory.exists():
        for file_path in directory.rglob('*'):
            if file_path.is_file():
                rel_path = file_path.relative_to(directory)
                hashes[str(rel_path)] = compute_file_hash(file_path)
    return hashes


def save_directory_hashes(directory: Path, hash_file: Path):
    """Save directory hashes to a file."""
    hashes = compute_directory_hash(directory)
    with open(hash_file, 'w') as f:
        json.dump(hashes, f, indent=2)


def load_directory_hashes(hash_file: Path) -> Optional[Dict[str, str]]:
    """Load directory hashes from a file."""
    if hash_file.exists():
        with open(hash_file, 'r') as f:
            return json.load(f)
    return None


def has_directory_changed(directory: Path, hash_file: Path) -> bool:
    """Check if directory content has changed since last hash save."""
    current_hashes = compute_directory_hash(directory)
    saved_hashes = load_directory_hashes(hash_file)
    
    if saved_hashes is None:
        return True
    
    return current_hashes != saved_hashes


def build_octos() -> bool:
    """Build octos binary with required features."""
    log.info("=" * 60)
    log.info("Building octos (telegram, discord)")
    log.info("=" * 60)
    
    build_log = LOG_DIR / "build.log"
    
    cmd = [
        "cargo", "build", "--release", "-p", "octos-cli",
        "--features", "telegram,discord"
    ]
    
    try:
        with open(build_log, "w") as f:
            result = subprocess.run(
                cmd,
                cwd=PROJECT_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            f.write(result.stdout)
            if result.returncode != 0:
                log.error("Build failed! Check log: %s", build_log)
                return False
    except Exception as e:
        log.error("Build failed: %s", e)
        return False
    
    if not BINARY_PATH.exists():
        log.error("Binary not found after build: %s", BINARY_PATH)
        return False
    
    log.info("✅ Build complete")
    return True


def prepare_test_environment():
    """Prepare test environment and copy files if needed."""
    TEST_DIR.mkdir(parents=True, exist_ok=True)
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    
    # Check if bot test files have changed
    bot_hash_file = TEST_DIR / ".bot_test_hashes.json"
    if has_directory_changed(BOT_TEST_DIR, bot_hash_file):
        log.info("Bot test files changed, updating...")
        # In a real scenario, you might copy files here
        # For now, we just track the hashes
        save_directory_hashes(BOT_TEST_DIR, bot_hash_file)
    else:
        log.info("Bot test files unchanged, skipping copy")
    
    # Check if CLI test files have changed
    cli_hash_file = TEST_DIR / ".cli_test_hashes.json"
    if has_directory_changed(CLI_TEST_DIR, cli_hash_file):
        log.info("CLI test files changed, updating...")
        save_directory_hashes(CLI_TEST_DIR, cli_hash_file)
    else:
        log.info("CLI test files unchanged, skipping copy")


def list_bot_modules():
    """List available bot test modules."""
    print("")
    print("Available bot test modules:")
    print("  telegram (tg)  - Telegram bot tests")
    print("  discord (dc)   - Discord bot tests")
    print("")


def list_bot_cases(module: str):
    """List test cases in a bot module."""
    test_files = {
        "telegram": BOT_TEST_DIR / "test_telegram.py",
        "tg": BOT_TEST_DIR / "test_telegram.py",
        "discord": BOT_TEST_DIR / "test_discord.py",
        "dc": BOT_TEST_DIR / "test_discord.py",
    }
    
    test_file = test_files.get(module)
    if not test_file or not test_file.exists():
        print(f"Test file not found for module: {module}")
        return
    
    # Use pytest to collect tests
    venv_python = BOT_TEST_DIR / ".venv" / "bin" / "python"
    if not venv_python.exists():
        print("Python venv not found. Run a test first to create it.")
        return
    
    result = subprocess.run(
        [str(venv_python), "-m", "pytest", str(test_file), "--collect-only", "-q"],
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONPATH": str(BOT_TEST_DIR)},
    )
    
    print(f"\nTest cases in {module}:")
    count = 0
    current_class = None
    for line in result.stdout.splitlines():
        if "::" in line and "test_" in line:
            # Parse class and test name from full path
            parts = line.split("::")
            if len(parts) >= 2:
                cls = parts[-2]
                func = parts[-1]
                
                # Print class header when it changes
                if cls != current_class:
                    current_class = cls
                    print(f"\n  {cls}")
                
                count += 1
                print(f"    {count}. {func}")
    
    if count == 0:
        print("No test cases found")
    else:
        print(f"\nTotal: {count} test(s)\n")


def run_bot_test(module: str, test_case: Optional[str] = None) -> Tuple[bool, List[str]]:
    """Run bot tests for a specific module.
    
    Returns:
        Tuple of (passed, failed_test_names)
    """
    module_logger = logger_mgr.get_module_logger(f"bot_{module}")
    
    module_logger.info("=" * 60)
    module_logger.info(f"Running {module} bot tests")
    module_logger.info("=" * 60)
    
    # Check environment
    required_vars = ["ANTHROPIC_API_KEY"]
    if module in ["telegram", "tg"]:
        required_vars.append("TELEGRAM_BOT_TOKEN")
    
    if not check_environment(required_vars):
        return False, []
    
    # Clean up any lingering octos processes from previous runs
    module_logger.info("Cleaning up lingering processes...")
    try:
        # Kill any running octos gateway processes
        result = subprocess.run(
            ["pkill", "-f", "octos gateway"],
            capture_output=True,
            timeout=5
        )
        if result.returncode == 0:
            module_logger.info("Killed lingering octos gateway processes")
        else:
            module_logger.debug("No lingering octos gateway processes found")
    except FileNotFoundError:
        module_logger.warning("pkill not found, skipping process cleanup")
    except subprocess.TimeoutExpired:
        module_logger.warning("Process cleanup timed out")
    except Exception as e:
        module_logger.warning(f"Failed to clean up processes: {e}")
    
    # Wait a moment for processes to fully terminate
    time.sleep(1)
    
    # Setup venv if needed
    venv_python = BOT_TEST_DIR / ".venv" / "bin" / "python"
    if not venv_python.exists():
        module_logger.info("Creating Python venv...")
        subprocess.run(["uv", "venv", str(BOT_TEST_DIR / ".venv")], check=True)
        subprocess.run([
            "uv", "pip", "install",
            "fastapi", "uvicorn", "httpx", "pytest", "pytest-asyncio", "websockets",
            "--python", str(venv_python),
        ], check=True)
    
    # Determine test file and module info
    module_info = {
        "telegram": {"port": 5000, "test_file": "test_telegram.py", "mock_module": "mock_tg", "mock_class": "MockTelegramServer"},
        "tg": {"port": 5000, "test_file": "test_telegram.py", "mock_module": "mock_tg", "mock_class": "MockTelegramServer"},
        "discord": {"port": 5001, "test_file": "test_discord.py", "mock_module": "mock_discord", "mock_class": "MockDiscordServer"},
        "dc": {"port": 5001, "test_file": "test_discord.py", "mock_module": "mock_discord", "mock_class": "MockDiscordServer"},
    }
    
    info = module_info.get(module)
    if not info:
        module_logger.error(f"Unknown module: {module}")
        return False, []
    
    port = info["port"]
    test_file = info["test_file"]
    mock_module = info["mock_module"]
    mock_class = info["mock_class"]
    
    test_path = BOT_TEST_DIR / test_file
    if not test_path.exists():
        module_logger.error(f"Test file not found: {test_path}")
        return False, []
    
    # Prepare config
    config_dir = TEST_DIR / ".octos"
    config_dir.mkdir(parents=True, exist_ok=True)
    config_file = config_dir / f"test_{module}_config.json"
    
    if module in ["telegram", "tg"]:
        extra_env = {"TELOXIDE_API_URL": f"http://127.0.0.1:{port}"}
        config = {
            "version": 1,
            "provider": "anthropic",
            "model": "MiniMax-M2.7",
            "api_key_env": "ANTHROPIC_API_KEY",
            "base_url": "https://api.minimaxi.com/anthropic",
            "gateway": {
                "channels": [{"type": "telegram", "settings": {"token_env": "TELEGRAM_BOT_TOKEN"}, "allowed_senders": []}],
            },
        }
    else:
        if not os.environ.get("DISCORD_BOT_TOKEN"):
            os.environ["DISCORD_BOT_TOKEN"] = "mock-bot-token-for-testing"
            module_logger.info("DISCORD_BOT_TOKEN not set, using dummy value (mock mode)")
        extra_env = {"DISCORD_API_BASE_URL": f"http://127.0.0.1:{port}"}
        config = {
            "version": 1,
            "provider": "anthropic",
            "model": "MiniMax-M2.7",
            "api_key_env": "ANTHROPIC_API_KEY",
            "base_url": "https://api.minimaxi.com/anthropic",
            "gateway": {
                "channels": [{"type": "discord", "settings": {"token_env": "DISCORD_BOT_TOKEN"}, "allowed_senders": []}],
            },
        }
    
    with open(config_file, "w") as f:
        json.dump(config, f, indent=2)
    
    # Clean up previous test artifacts
    module_logger.info("Cleaning up previous test artifacts...")
    
    # Kill any existing processes on the port
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
                    module_logger.info(f"Killed process {pid} on port {port}")
                except ProcessLookupError:
                    pass
        if pids:
            time.sleep(1)
    except Exception:
        pass
    
    # Remove database lock files
    db_files = list(TEST_DIR.glob("*.redb")) + list(TEST_DIR.glob("*.redb.lock"))
    for db_file in db_files:
        try:
            db_file.unlink()
            module_logger.info(f"Removed database file: {db_file}")
        except FileNotFoundError:
            pass
    
    # Clear Python cache
    import shutil
    for pattern in ["__pycache__", "*.pyc", ".pytest_cache"]:
        for path in BOT_TEST_DIR.glob(f"**/{pattern}"):
            if path.is_dir():
                shutil.rmtree(path, ignore_errors=True)
            elif path.is_file():
                path.unlink(missing_ok=True)
    
    # Start Mock Server
    timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
    bot_log = LOG_DIR / f"02_gateway_{module}_{timestamp}.log"
    
    mock_code = f"""
import time, signal, sys, logging
from {mock_module} import {mock_class}
logging.getLogger("httpx").setLevel(logging.WARNING)
logging.getLogger("serenity").setLevel(logging.ERROR)  # Suppress stream edit warnings
server = {mock_class}(port={port})
server.start_background(log_file='{bot_log}')
print('ready', flush=True)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
while True:
    time.sleep(1)
"""
    
    mock_proc = subprocess.Popen(
        [str(venv_python), "-c", mock_code],
        env={**os.environ, "PYTHONPATH": str(BOT_TEST_DIR), "PYTHONDONTWRITEBYTECODE": "1"},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    
    # Wait for mock server to be ready
    health_timeout = 5 if module in ["discord", "dc"] else 3
    start = time.time()
    mock_ready = False
    last_error = None
    
    while time.time() - start < health_timeout:
        # Check if process has exited
        if mock_proc.poll() is not None:
            # Process exited, read output for error
            stdout, _ = mock_proc.communicate()
            last_error = stdout.decode('utf-8', errors='ignore') if stdout else "Unknown error"
            break
        
        try:
            resp = httpx.get(f"http://127.0.0.1:{port}/health", timeout=2)
            if resp.status_code == 200:
                mock_ready = True
                break
        except Exception as e:
            last_error = str(e)
        time.sleep(0.5)
    
    if not mock_ready:
        module_logger.error(f"{module} Mock server failed to start")
        if last_error:
            module_logger.error(f"Error details: {last_error}")
        
        # Try to read any remaining output
        try:
            mock_proc.terminate()
            stdout, _ = mock_proc.communicate(timeout=2)
            if stdout:
                output = stdout.decode('utf-8', errors='ignore')
                module_logger.error(f"Mock server output:\n{output}")
        except Exception:
            pass
        
        return False, []
    
    mock_pid = mock_proc.pid
    module_logger.info(f"{module} Mock server running on port {port} (PID {mock_pid})")
    
    # Start Octos Gateway
    if not BINARY_PATH.exists():
        module_logger.error(f"Octos binary not found: {BINARY_PATH}")
        mock_proc.terminate()
        return False, []
    
    bot_env = {**os.environ, **extra_env}
    
    # Open log file for bot output
    bot_log_file = open(bot_log, 'w')
    
    bot_proc = subprocess.Popen(
        [str(BINARY_PATH), "gateway", "--config", str(config_file), "--data-dir", str(TEST_DIR)],
        env=bot_env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    bot_pid = bot_proc.pid
    module_logger.info(f"Bot PID: {bot_pid}")
    
    # Start a thread to read bot output and tee to file + stdout
    import threading
    import select
    import sys  # Import sys inside the function scope
    
    def tee_output(proc, log_file):
        """Read process output and write to both file and stdout.
        
        Uses non-blocking reads with timeout to avoid blocking indefinitely.
        Handles file closure gracefully.
        """
        try:
            while True:
                # Check if process is still running
                if proc.poll() is not None:
                    # Process ended, read any remaining output
                    remaining = proc.stdout.read()
                    if remaining:
                        text = remaining.decode('utf-8', errors='ignore')
                        try:
                            log_file.write(text)
                            log_file.flush()
                        except (ValueError, IOError):
                            pass  # File already closed, skip
                        sys.stdout.write(text)
                        sys.stdout.flush()
                    break
                
                # Try to read a line with a short timeout
                line = proc.stdout.readline()
                if not line:  # EOF or empty
                    time.sleep(0.1)  # Small delay to avoid busy-waiting
                    continue
                
                text = line.decode('utf-8', errors='ignore')
                # Write to file (handle closed file gracefully)
                try:
                    log_file.write(text)
                    log_file.flush()
                except (ValueError, IOError):
                    pass  # File already closed, continue writing to stdout only
                
                # Write to stdout
                sys.stdout.write(text)
                sys.stdout.flush()
        except Exception as e:
            # Only log unexpected errors, not file closure issues
            if not isinstance(e, (ValueError, IOError)):
                module_logger.error(f"Tee thread error: {e}")
                import traceback
                traceback.print_exc()
    
    tee_thread = threading.Thread(target=tee_output, args=(bot_proc, bot_log_file), daemon=True)
    tee_thread.start()
    
    # Wait for gateway to start
    module_logger.info("Waiting for gateway to start...")
    
    ready = False
    max_wait = 50 if module in ["discord", "dc"] else 40
    start = time.time()
    
    try:
        while time.time() - start < max_wait:
            if bot_proc.poll() is not None:
                module_logger.error("Bot process exited unexpectedly")
                break
            
            # Check both file and captured output
            found_ready = False
            try:
                with open(bot_log) as f:
                    content = f.read()
                    if re.search(r"gateway.*ready|Gateway ready|\[gateway\] ready", content):
                        found_ready = True
            except FileNotFoundError:
                pass
            
            if found_ready:
                # Give tee thread a moment to finish writing
                time.sleep(0.5)
                ready = True
                break
            
            time.sleep(1)
    finally:
        def cleanup():
            # Step 1: Terminate processes first (tee thread will detect EOF)
            try:
                os.kill(bot_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                os.kill(mock_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            
            # Step 2: Wait for processes to exit and tee thread to finish
            time.sleep(1)
            
            # Force kill if still running
            try:
                os.kill(bot_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                os.kill(mock_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            
            # Step 3: Wait for tee thread to finish (it should exit when process ends)
            try:
                tee_thread.join(timeout=3)
            except Exception:
                pass
            
            # Step 4: Close log file AFTER tee thread has finished
            try:
                bot_log_file.close()
            except Exception:
                pass
    
    if not ready:
        module_logger.error("Bot failed to start. Full log:")
        try:
            with open(bot_log) as f:
                for line in f:
                    module_logger.info(f"    {line.rstrip()}")
        except FileNotFoundError:
            pass
        cleanup()
        return False, []
    
    module_logger.info("Gateway ready!")
    
    # Run pytest
    pytest_args = [
        str(venv_python), "-m", "pytest",
        str(test_path),
        "--tb=line", "--no-header", "-p", "no:warnings",
        "--log-cli-level=DEBUG",  # Show all debug logs for troubleshooting
        "--color=yes",  # Enable colored output for better readability
        "-q",  # Quiet mode: show progress number on the left
    ]
    
    if test_case:
        pytest_args.extend(["-k", test_case])
        module_logger.info(f"Running specific test: {test_case}")
    
    module_logger.info(f"Executing: {' '.join(pytest_args)}")
    
    # Start pytest process
    pytest_proc = subprocess.Popen(
        pytest_args,
        env={**os.environ, "PYTHONPATH": str(BOT_TEST_DIR), "MOCK_BASE_URL": f"http://127.0.0.1:{port}"},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    
    # Read pytest output line by line and log it
    # This ensures unified timestamp format and perfect ordering
    import sys
    failed_tests = []  # Collect failed test names
    error_messages = []  # Collect error messages for unknown failures
    while True:
        # Check if Mock Server is still alive
        if mock_proc.poll() is not None:
            module_logger.error("❌ Mock Server process exited unexpectedly during tests!")
            try:
                stdout, _ = mock_proc.communicate(timeout=2)
                if stdout:
                    module_logger.error(f"Mock Server last output:\n{stdout.decode('utf-8', errors='ignore')}")
            except Exception:
                pass
            break
        
        # Check if Bot is still alive
        if bot_proc.poll() is not None:
            module_logger.error("❌ Bot process exited unexpectedly during tests!")
            break
            
        line = pytest_proc.stdout.readline()
        if not line:
            if pytest_proc.poll() is not None:
                break
            time.sleep(0.1)
            continue
            
        text = line.decode('utf-8', errors='ignore').rstrip()
        if text:
            module_logger.info(f"[PYTEST] {text}")
            # Detect failed tests from pytest output
            # Format: "test_file.py::test_name FAILED"
            if 'FAILED' in text and '::' in text:
                # Extract test name
                parts = text.split()
                for part in parts:
                    if '::' in part and part.endswith('.py') == False:
                        # Get the test function name
                        test_name = part.split('::')[-1]
                        if test_name not in failed_tests:
                            failed_tests.append(test_name)
                        break
            # Also capture ERROR lines that might indicate setup failures
            elif 'ERROR' in text and ('test_' in text or 'setup' in text.lower()):
                error_messages.append(text)
    
    pytest_proc.wait()
    result = subprocess.CompletedProcess(
        args=pytest_args,
        returncode=pytest_proc.returncode,
    )
    
    cleanup()
    
    if result.returncode == 0:
        module_logger.info(f"✅ All {module} tests passed!")
    else:
        module_logger.error(f"❌ Some {module} tests failed")
        if failed_tests:
            module_logger.error(f"Failed tests: {', '.join(failed_tests)}")
        elif error_messages:
            # If we have error messages but no specific test names, use those
            module_logger.error(f"Errors detected: {len(error_messages)} issues")
            for err_msg in error_messages[:5]:  # Show first 5 errors
                module_logger.error(f"  - {err_msg}")
    
    # Return failed tests, or error messages if no tests were captured
    return result.returncode == 0, failed_tests if failed_tests else error_messages


def run_all_bot_tests() -> Tuple[bool, List[str]]:
    """Run all bot tests (telegram + discord).
    
    Returns:
        Tuple of (all_passed, error_messages)
    """
    log.info("=" * 60)
    log.info("Running ALL bot tests")
    log.info("=" * 60)
    
    modules = ["telegram", "discord"]
    all_passed = True
    errors = []
    
    for module in modules:
        passed, failures = run_bot_test(module)
        if not passed:
            all_passed = False
            # Add detailed error messages for each failed test
            if failures:
                for failure in failures:
                    # If it's an error message (not a test name), format it differently
                    if '::' in failure or failure.startswith('test_'):
                        errors.append(f"{module}: {failure}")
                    else:
                        # It's an error message, truncate if too long
                        error_summary = failure[:100] + "..." if len(failure) > 100 else failure
                        errors.append(f"{module}: {error_summary}")
            else:
                errors.append(f"{module}: (unknown failures - check logs)")
    
    return all_passed, errors


def list_cli_categories():
    """List CLI test categories."""
    test_cases_file = CLI_TEST_DIR / "test_cases.json"
    if not test_cases_file.exists():
        print(f"Test cases file not found: {test_cases_file}")
        return
    
    with open(test_cases_file, 'r') as f:
        test_cases = json.load(f)
    
    print("\nAvailable CLI test categories:")
    for category in test_cases.keys():
        print(f"  - {category}")
    print("")


def list_cli_category_cases(category: str):
    """List test cases in a CLI category."""
    test_cases_file = CLI_TEST_DIR / "test_cases.json"
    if not test_cases_file.exists():
        print(f"Test cases file not found: {test_cases_file}")
        return
    
    with open(test_cases_file, 'r') as f:
        test_cases = json.load(f)
    
    if category not in test_cases:
        print(f"Unknown category: {category}")
        return
    
    cases = test_cases[category]
    
    # Only list if it's a list type
    if not isinstance(cases, list):
        print(f"\n'{category}' is not a test category (type: {type(cases).__name__})")
        print(f"Value: {cases}")
        print("")
        return
    
    print(f"\nTest cases in '{category}':")
    for idx, case in enumerate(cases, 1):
        # Handle both string and dict formats
        if isinstance(case, str):
            print(f"  {idx}. {case}")
        elif isinstance(case, dict):
            print(f"  {idx}. {case.get('name', 'Unnamed')}")
        else:
            print(f"  {idx}. {case}")
    print(f"\nTotal: {len(cases)} test(s)\n")


def run_cli_tests(verbose: bool = False, output_dir: Optional[str] = None, scope: Optional[str] = None) -> Tuple[bool, List[str]]:
    """Run CLI tests using the Python implementation.
    
    Returns:
        Tuple of (all_passed, error_messages)
    """
    cli_logger = logger_mgr.get_module_logger("cli")
    
    cli_logger.info("=" * 60)
    cli_logger.info("Running CLI tests")
    cli_logger.info("=" * 60)
    
    # Convert output_dir to Path if provided
    output_path = Path(output_dir) if output_dir else None
    
    try:
        # Call the Python CLI test module
        all_passed, errors = run_cli_tests_module(
            binary_path=BINARY_PATH,
            test_dir=TEST_DIR,
            log_dir=LOG_DIR,
            verbose=verbose,
            output_dir=output_path,
            scope=scope
        )
        
        if all_passed:
            cli_logger.info("✅ CLI tests passed")
        else:
            cli_logger.error("❌ CLI tests failed")
        
        return all_passed, errors
    except Exception as e:
        cli_logger.error(f"CLI tests failed with exception: {e}")
        import traceback
        cli_logger.error(traceback.format_exc())
        return False, [f"CLI tests exception: {str(e)}"]


def parse_args():
    """Parse command line arguments."""
    # Manual parsing to handle complex nested arguments
    args = sys.argv[1:]
    
    if not args or '-h' in args or '--help' in args:
        return argparse.Namespace(
            command='help' if args else None,
            test_target=None,
            remaining=[]
        )
    
    command = args[0]
    
    if command == '--test':
        if len(args) < 2:
            return argparse.Namespace(
                command='--test',
                test_target=None,
                remaining=[]
            )
        
        test_target = args[1]
        remaining = args[2:]
        
        return argparse.Namespace(
            command='--test',
            test_target=test_target,
            remaining=remaining
        )
    
    # For 'all' command
    return argparse.Namespace(
        command=command,
        test_target=None,
        remaining=args[1:]
    )


def main() -> int:
    """Main entry point."""
    args = parse_args()
    
    # Handle help - use print directly, no timestamp
    if args.command == 'help':
        print_help()
        return 0
    
    # No command provided - use print directly, no timestamp
    if args.command is None:
        print_help()
        return 0
    
    # Validate command
    valid_commands = ["all", "--test"]
    if args.command not in valid_commands:
        log.error(f"Unknown command: {args.command}")
        print_help()
        return 1
    
    # Handle --test command
    if args.command == "--test":
        test_target = args.test_target
        
        if not test_target:
            log.error("Missing test target. Use: --test bot or --test cli")
            print_help()
            return 1
        
        if test_target not in ["bot", "cli"]:
            log.error(f"Unknown test target: {test_target}")
            print_help()
            return 1
        
        remaining = args.remaining
        
        # Handle bot tests
        if test_target == "bot":
            if not remaining:
                # Default: run all bot tests
                if not build_octos():
                    return 1
                prepare_test_environment()
                passed, _ = run_all_bot_tests()
                return 0 if passed else 1
            
            action = remaining[0]
            
            # Help for bot
            if action in ["-h", "--help"]:
                print_help()
                return 0
            
            # List modules
            if action in ["list", "ls"]:
                if len(remaining) > 1:
                    list_bot_cases(remaining[1])
                else:
                    list_bot_modules()
                return 0
            
            # Run specific module or all
            if action == "all":
                if not build_octos():
                    return 1
                prepare_test_environment()
                passed, _ = run_all_bot_tests()
                return 0 if passed else 1
            
            # Check if it's a valid module
            valid_modules = ["telegram", "tg", "discord", "dc"]
            if action in valid_modules:
                # Special case: check for 'list' subcommand
                if len(remaining) > 1 and remaining[1] == "list":
                    list_bot_cases(action)
                    return 0
                
                if not build_octos():
                    return 1
                prepare_test_environment()
                test_case = remaining[1] if len(remaining) > 1 else None
                passed, _ = run_bot_test(action, test_case)
                return 0 if passed else 1
            
            log.error(f"Unknown bot argument: {action}")
            print_help()
            return 1
        
        # Handle CLI tests
        elif test_target == "cli":
            if not remaining:
                # Default: run CLI tests
                if not build_octos():
                    return 1
                prepare_test_environment()
                passed, _ = run_cli_tests()
                return 0 if passed else 1
            
            action = remaining[0]
            
            # Help for CLI
            if action in ["-h", "--help"]:
                print_help()
                return 0
            
            # List categories
            if action == "list":
                if len(remaining) > 1:
                    list_cli_category_cases(remaining[1])
                else:
                    list_cli_categories()
                return 0
            
            # Parse CLI options
            verbose = "-v" in remaining or "--verbose" in remaining
            output_dir = None
            scope = None
            
            for i, arg in enumerate(remaining):
                if arg in ["-o", "--output-dir"] and i + 1 < len(remaining):
                    output_dir = remaining[i + 1]
                elif arg in ["-s", "--scope"] and i + 1 < len(remaining):
                    scope = remaining[i + 1]
            
            if not build_octos():
                return 1
            prepare_test_environment()
            passed, _ = run_cli_tests(verbose, output_dir, scope)
            return 0 if passed else 1
    
    # Handle 'all' command
    if args.command == "all":
        # Check for invalid extra arguments
        if args.remaining:
            log.error(f"Invalid arguments for 'all' command: {' '.join(args.remaining)}")
            print("")
            print("The 'all' command runs all tests and does not accept additional arguments.")
            print("")
            print("Usage:")
            print("  tests/test_run.py all                    # Run all tests")
            print("  tests/test_run.py --test bot [args...]   # Run bot tests with options")
            print("  tests/test_run.py --test cli [args...]   # Run CLI tests with options")
            print("")
            print_help()
            return 1
        
        # Use print instead of log for final report
        print("")
        print("=" * 70)
        print("🚀 Running ALL Test Suites (CLI + Bot)")
        print("=" * 70)
        print("")
        
        if not build_octos():
            return 1
        
        prepare_test_environment()
        
        # Run tests in order: CLI first, then Bot
        cli_passed, cli_errors = run_cli_tests()
        bot_passed, bot_errors = run_all_bot_tests()
        
        # Generate comprehensive report using print (not log)
        print("")
        print("=" * 70)
        print("📊 TEST SUMMARY REPORT")
        print("=" * 70)
        print("")
        print(f"📅 Test Date: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
        print("")
        
        # Module status
        print("🔹 Module Status:")
        print(f"   • CLI Tests:   {'✅ PASSED' if cli_passed else '❌ FAILED'}")
        print(f"   • Bot Tests:   {'✅ PASSED' if bot_passed else '❌ FAILED'}")
        print("")
        
        # Overall result
        overall_passed = cli_passed and bot_passed
        print(f"🎯 Overall Result: {'✅ ALL TESTS PASSED' if overall_passed else '❌ SOME TESTS FAILED'}")
        print("")
        
        # Error details
        has_errors = len(cli_errors) > 0 or len(bot_errors) > 0
        if has_errors:
            print("=" * 70)
            print("❌ FAILED TESTS DETAILS")
            print("=" * 70)
            print("")
            
            if cli_errors:
                print("🔸 CLI Test Failures:")
                for error in cli_errors:
                    print(f"   • {error}")
                print("")
            
            if bot_errors:
                print("🔸 Bot Test Failures:")
                for error in bot_errors:
                    print(f"   • {error}")
                print("")
            
            print(f"📝 Total Failures: {len(bot_errors) + len(cli_errors)}")
            print("")
        
        # Log location
        print("=" * 70)
        print(f"📁 Logs: {LOG_DIR}")
        print("=" * 70)
        print("")
        
        return 0 if overall_passed else 1
    
    return 1


if __name__ == "__main__":
    sys.exit(main())
