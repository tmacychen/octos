#!/usr/bin/env python3
"""
Test Runner for Telegram Bot Tests

This module provides utilities to run integration tests for the octos bot
using the mock Telegram API server.

Usage:
    python -m telegram_mock.runner
"""

import asyncio
import os
import sys
import time
import signal
from pathlib import Path
from typing import Callable, Any

# Add project root to path
project_root = Path(__file__).parent.parent.parent
sys.path.insert(0, str(project_root))

from telegram_mock import MockTelegramServer


class BotTestRunner:
    """
    Test runner that manages the mock server and bot process.
    
    Usage:
        runner = BotTestRunner()
        await runner.start()
        
        # Inject messages and check responses
        runner.server.inject_message("/start")
        await asyncio.sleep(2)
        
        # Check results
        messages = runner.server.get_sent_messages()
        assert any("Welcome" in m.text for m in messages)
        
        await runner.stop()
    """
    
    def __init__(self, bot_token: str = "test_token_123", 
                 mock_port: int = 5000,
                 bot_port: int = 8080):
        self.bot_token = bot_token
        self.mock_port = mock_port
        self.bot_port = bot_port
        self.server = MockTelegramServer(port=mock_port)
        self.bot_process = None
        self._running = False
    
    async def start(self, env_overrides: dict = None):
        """Start the mock server and bot"""
        # Start mock server in background
        self.server.start_background()
        await asyncio.sleep(1)  # Wait for server to start
        
        # Set environment variables for the bot
        env = os.environ.copy()
        env["TELOXIDE_API_URL"] = f"http://127.0.0.1:{self.mock_port}"
        env["TELOXIDE_TOKEN"] = self.bot_token
        if env_overrides:
            env.update(env_overrides)
        
        # TODO: Start the actual bot process
        # For now, this is a placeholder - the bot would be started via:
        # self.bot_process = subprocess.Popen(
        #     ["cargo", "run", "--", "--telegram-token", self.bot_token],
        #     env=env,
        #     cwd=project_root
        # )
        
        self._running = True
        print(f"✅ Test runner started (mock: {self.mock_port}, bot port: {self.bot_port})")
    
    async def stop(self):
        """Stop the mock server and bot"""
        if self.bot_process:
            self.bot_process.terminate()
            self.bot_process.wait()
        
        self._running = False
        print("🛑 Test runner stopped")
    
    def inject_and_wait(self, text: str, wait_seconds: float = 2.0):
        """Inject a message and wait for bot to process"""
        self.server.inject_message(text)
        time.sleep(wait_seconds)
    
    def get_responses(self) -> list:
        """Get all messages sent by the bot"""
        return self.server.get_sent_messages()
    
    def find_response(self, predicate: Callable[[Any], bool]) -> Any:
        """Find a response matching a predicate"""
        for msg in self.server.get_sent_messages():
            if predicate(msg):
                return msg
        return None


async def run_simple_test():
    """Run a simple test to verify the mock server works"""
    print("🧪 Running simple mock server test...")
    
    server = MockTelegramServer()
    server.start_background()
    await asyncio.sleep(1)
    
    # Test 1: Inject a message
    server.inject_message("/start", chat_id=123)
    await asyncio.sleep(0.5)
    
    # Test 2: Simulate bot sending a response (for testing the mock itself)
    # In real tests, the bot would call sendMessage, but we can verify the endpoint works
    
    # Test 3: Check health endpoint
    import httpx
    async with httpx.AsyncClient() as client:
        resp = await client.get(f"http://127.0.0.1:{server.port}/health")
        assert resp.status_code == 200
        print("✅ Health check passed")
    
    print("✅ Simple test passed!")
    return True


def main():
    """Main entry point"""
    import argparse
    
    parser = argparse.ArgumentParser(description="Telegram Bot Test Runner")
    parser.add_argument("--test", action="store_true", help="Run built-in tests")
    parser.add_argument("--port", type=int, default=5000, help="Mock server port")
    args = parser.parse_args()
    
    if args.test:
        asyncio.run(run_simple_test())
    else:
        print("Telegram Mock Test Runner")
        print("=" * 40)
        print("Use --test to run built-in tests")
        print("Import MockTelegramServer in your own test code")


if __name__ == "__main__":
    main()