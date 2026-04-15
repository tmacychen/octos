"""
Bot Mock Testing Framework (Telegram & Discord)

This package provides tools for testing the octos bot
without requiring real Telegram or Discord API credentials.
"""

from .mock_tg import MockTelegramServer
from .mock_discord import MockDiscordServer
from .base_runner import BaseMockRunner

__all__ = ["MockTelegramServer", "MockDiscordServer", "BaseMockRunner"]
