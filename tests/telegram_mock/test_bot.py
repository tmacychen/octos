#!/usr/bin/env python3
"""
Pytest-based tests for the Telegram bot

These tests demonstrate how to use the mock server to test bot functionality.
Run with: pytest tests/telegram_mock/test_bot.py -v

Prerequisites:
    pip install pytest pytest-asyncio httpx
    cargo build --release  # Build the octos binary first
"""

import asyncio
import os
import subprocess
import time
import pytest
import httpx
from typing import Generator

# Import the mock server
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from telegram_mock import MockTelegramServer


# Configuration
MOCK_PORT = 5000
BOT_PORT = 8080
TEST_TOKEN = "test_token_123456"
BOT_BINARY = "target/release/octos-bus"  # Adjust as needed


@pytest.fixture(scope="module")
def mock_server() -> Generator[MockTelegramServer, None, None]:
    """Fixture that provides a mock Telegram server"""
    server = MockTelegramServer(port=MOCK_PORT)
    server.start_background()
    time.sleep(1)  # Wait for server to start
    yield server
    server.clear()


@pytest.fixture(scope="module")
def bot_process(mock_server: MockTelegramServer) -> Generator[subprocess.Popen, None, None]:
    """Fixture that starts the bot process with mock server configuration"""
    
    # Check if bot binary exists
    if not os.path.exists(BOT_BINARY):
        pytest.skip(f"Bot binary not found at {BOT_BINARY}. Run 'cargo build --release' first.")
    
    # Set environment for bot to use mock server
    env = os.environ.copy()
    env["TELOXIDE_API_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
    env["TELOXIDE_TOKEN"] = TEST_TOKEN
    
    # TODO: Add your bot's required environment variables here
    # env["OCTOS_CONFIG_PATH"] = "config.test.json"
    
    # Start the bot
    # Note: This is a placeholder - adjust based on your bot's CLI
    process = subprocess.Popen(
        [BOT_BINARY, "--telegram-token", TEST_TOKEN],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    
    # Wait for bot to initialize
    time.sleep(3)
    
    yield process
    
    # Cleanup
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()


class TestTelegramBot:
    """Test suite for Telegram bot functionality"""
    
    def test_mock_server_health(self, mock_server: MockTelegramServer):
        """Test that the mock server is running"""
        import httpx
        
        # Note: Can't use httpx.AsyncClient directly in sync test
        # This is just a placeholder - real tests would be async
        assert mock_server is not None
        assert mock_server.port == MOCK_PORT
    
    @pytest.mark.asyncio
    async def test_bot_responds_to_start_command(self, mock_server: MockTelegramServer):
        """Test that bot responds to /start command"""
        
        # Inject /start message from user
        mock_server.inject_message("/start", chat_id=123, from_username="testuser")
        
        # Wait for bot to process
        await asyncio.sleep(2)
        
        # Check that bot sent a message
        sent_messages = mock_server.get_sent_messages()
        assert len(sent_messages) > 0, "Bot should have sent at least one message"
        
        # Check that the message contains welcome text
        last_message = sent_messages[-1]
        assert last_message.text is not None
        print(f"📝 Bot response: {last_message.text}")
    
    @pytest.mark.asyncio
    async def test_bot_handles_regular_text(self, mock_server: MockTelegramServer):
        """Test that bot handles regular text messages"""
        
        # Clear previous messages
        mock_server.clear()
        
        # Inject a regular text message
        mock_server.inject_message("Hello bot!", chat_id=123, from_username="testuser")
        
        # Wait for bot to process
        await asyncio.sleep(2)
        
        # Check bot response
        sent_messages = mock_server.get_sent_messages()
        assert len(sent_messages) > 0, "Bot should respond to text messages"
    
    @pytest.mark.asyncio
    async def test_bot_handles_callback_query(self, mock_server: MockTelegramServer):
        """Test that bot handles callback queries (button presses)"""
        
        mock_server.clear()
        
        # Inject a callback query (button press)
        mock_server.inject_callback_query("s:topic1", chat_id=123, message_id=100)
        
        # Wait for bot to process
        await asyncio.sleep(2)
        
        # Check bot response
        sent_messages = mock_server.get_sent_messages()
        # Bot might respond to callback or not depending on implementation
        print(f"📝 Callback handled, messages: {len(sent_messages)}")


# --- Manual test runner (for development) ---

async def run_manual_test():
    """
    Manual test runner for development.
    Run this to test the bot without pytest.
    """
    print("=" * 60)
    print("Telegram Bot Manual Test")
    print("=" * 60)
    
    # Start mock server
    server = MockTelegramServer(port=MOCK_PORT)
    server.start_background()
    await asyncio.sleep(1)
    
    # Check health
    async with httpx.AsyncClient() as client:
        resp = await client.get(f"http://127.0.0.1:{MOCK_PORT}/health")
        print(f"✅ Mock server health: {resp.json()}")
    
    # Test injecting messages
    print("\n📥 Injecting test messages...")
    
    # Test 1: /start command
    server.inject_message("/start", chat_id=123, from_username="testuser")
    await asyncio.sleep(1)
    
    # Test 2: Regular message
    server.inject_message("Hello!", chat_id=123, from_username="testuser")
    await asyncio.sleep(1)
    
    # Test 3: Callback query
    server.inject_callback_query("s:topic1", chat_id=123, message_id=100)
    await asyncio.sleep(1)
    
    # Show results
    print("\n📤 Messages sent by bot:")
    for i, msg in enumerate(server.get_sent_messages(), 1):
        print(f"  {i}. Chat {msg.chat_id}: {msg.text[:80]}...")
    
    print("\n✅ Manual test complete!")


if __name__ == "__main__":
    import sys
    
    if len(sys.argv) > 1 and sys.argv[1] == "--manual":
        asyncio.run(run_manual_test())
    else:
        print("Run tests with: pytest tests/telegram_mock/test_bot.py -v")
        print("Or run manual test: python -m telegram_mock.test_bot --manual")