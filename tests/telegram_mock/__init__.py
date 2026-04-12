"""
Telegram Mock Testing Framework

This package provides tools for testing the octos Telegram bot
without requiring real Telegram API credentials.

Usage:
    from telegram_mock import MockTelegramServer, run_bot_test
    
    # Start mock server and run a test
    server = MockTelegramServer()
    server.start_background()
    
    # ... run bot and test
"""

from .mock_tg import MockTelegramServer

__all__ = ["MockTelegramServer"]