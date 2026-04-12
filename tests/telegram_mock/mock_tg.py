#!/usr/bin/env python3
"""
Telegram API Mock Server for Testing

This module provides a mock Telegram API server that simulates Telegram's API
for testing purposes. It allows testing the octos bot without needing real
Telegram API credentials.

Usage:
    # Start the mock server
    python -m telegram_mock.mock_tg

    # Or import and use programmatically
    from telegram_mock import MockTelegramServer
    server = MockTelegramServer()
    await server.start()
"""

import asyncio
import uuid
from dataclasses import dataclass, field
from typing import Any
from fastapi import FastAPI, Request, HTTPException
import uvicorn
from threading import Thread
import logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


@dataclass
class Update:
    """Represents a Telegram update (message, callback, etc.)"""
    update_id: int
    message: dict = field(default_factory=dict)
    callback_query: dict = field(default_factory=dict)
    edited_message: dict = field(default_factory=dict)
    channel_post: dict = field(default_factory=dict)


@dataclass
class SentMessage:
    """Represents a message sent by the bot"""
    chat_id: int
    text: str
    parse_mode: str = "Markdown"
    reply_markup: dict = field(default_factory=dict)
    reply_to_message_id: int = None


class MockTelegramServer:
    """
    Mock Telegram API Server
    
    Simulates the Telegram Bot API for testing purposes.
    Implements the most common endpoints used by the octos bot.
    """
    
    def __init__(self, host: str = "127.0.0.1", port: int = 5000):
        self.host = host
        self.port = port
        self.app = FastAPI(title="Telegram Mock API")
        self._setup_routes()
        
        # Storage
        self._updates: list[Update] = []
        self._sent_messages: list[SentMessage] = []
        self._next_update_id = 1
        self._bot_info = {
            "id": 123456789,
            "is_bot": True,
            "first_name": "TestBot",
            "username": "test_octos_bot",
            "can_join_groups": True,
            "can_read_all_group_messages": False,
            "supports_inline_queries": False,
        }
        self._commands_registered = []
        
    def _setup_routes(self):
        """Set up FastAPI routes"""
        app = self.app
        
        @app.get("/bot{token}/getUpdates")
        async def get_updates(token: str, offset: int = 0, timeout: int = 30):
            """Long polling endpoint - returns pending updates"""
            # Filter updates with ID > offset
            updates = [u for u in self._updates if u.update_id > offset]
            
            # If no updates and timeout > 0, wait a bit (simulate real API)
            if not updates and timeout > 0:
                await asyncio.sleep(min(timeout, 1))
                updates = [u for u in self._updates if u.update_id > offset]
            
            return {"ok": True, "result": self._serialize_updates(updates)}
        
        @app.post("/bot{token}/sendMessage")
        async def send_message(token: str, request: Request):
            """Send a message - bot calls this to reply to users"""
            data = await request.json()
            
            chat_id = data.get("chat_id")
            text = data.get("text", "")
            parse_mode = data.get("parse_mode", "Markdown")
            reply_markup = data.get("reply_markup")
            reply_to_message_id = data.get("reply_to_message_id")
            
            if not chat_id:
                raise HTTPException(status_code=400, detail="chat_id is required")
            
            message_id = self._next_update_id + 1000
            self._sent_messages.append(SentMessage(
                chat_id=chat_id,
                text=text,
                parse_mode=parse_mode,
                reply_markup=reply_markup or {},
                reply_to_message_id=reply_to_message_id,
            ))
            
            logger.info(f"📤 Bot sent message to {chat_id}: {text[:50]}...")
            
            return {
                "ok": True,
                "result": {
                    "message_id": message_id,
                    "from": self._bot_info,
                    "chat": {"id": chat_id, "type": "private"},
                    "text": text,
                    "date": 1234567890,
                }
            }
        
        @app.post("/bot{token}/sendDocument")
        async def send_document(token: str, request: Request):
            """Send a document"""
            data = await request.json()
            chat_id = data.get("chat_id")
            
            message_id = self._next_update_id + 1000
            self._sent_messages.append(SentMessage(
                chat_id=chat_id,
                text="[document]",
                parse_mode="Markdown",
            ))
            
            return {
                "ok": True,
                "result": {
                    "message_id": message_id,
                    "from": self._bot_info,
                    "chat": {"id": chat_id, "type": "private"},
                    "document": {"file_name": "test.pdf", "file_id": "abc123"},
                }
            }
        
        @app.post("/bot{token}/sendVoice")
        async def send_voice(token: str, request: Request):
            """Send a voice message"""
            data = await request.json()
            chat_id = data.get("chat_id")
            
            message_id = self._next_update_id + 1000
            self._sent_messages.append(SentMessage(
                chat_id=chat_id,
                text="[voice]",
                parse_mode="Markdown",
            ))
            
            return {
                "ok": True,
                "result": {
                    "message_id": message_id,
                    "from": self._bot_info,
                    "chat": {"id": chat_id, "type": "private"},
                    "voice": {"file_id": "voice123", "duration": 10},
                }
            }
        
        @app.post("/bot{token}/sendAudio")
        async def send_audio(token: str, request: Request):
            """Send an audio file"""
            data = await request.json()
            chat_id = data.get("chat_id")
            
            message_id = self._next_update_id + 1000
            self._sent_messages.append(SentMessage(
                chat_id=chat_id,
                text="[audio]",
                parse_mode="Markdown",
            ))
            
            return {
                "ok": True,
                "result": {
                    "message_id": message_id,
                    "from": self._bot_info,
                    "chat": {"id": chat_id, "type": "private"},
                    "audio": {"file_id": "audio123", "duration": 60},
                }
            }
        
        @app.post("/bot{token}/editMessageText")
        async def edit_message_text(token: str, request: Request):
            """Edit a message"""
            data = await request.json()
            return {
                "ok": True,
                "result": {
                    "message_id": data.get("message_id"),
                    "chat": {"id": data.get("chat_id")},
                    "text": data.get("text"),
                }
            }
        
        @app.post("/bot{token}/deleteMessage")
        async def delete_message(token: str, request: Request):
            """Delete a message"""
            return {"ok": True, "result": True}
        
        @app.post("/bot{token}/answerCallbackQuery")
        async def answer_callback_query(token: str, request: Request):
            """Answer a callback query (dismiss loading spinner)"""
            return {"ok": True, "result": True}
        
        @app.post("/bot{token}/getMe")
        async def get_me(token: str):
            """Get bot info"""
            return {"ok": True, "result": self._bot_info}
        
        @app.post("/bot{token}/setMyCommands")
        async def set_my_commands(token: str, request: Request):
            """Register bot commands"""
            data = await request.json()
            self._commands_registered = data.get("commands", [])
            logger.info(f"📝 Bot registered commands: {self._commands_registered}")
            return {"ok": True, "result": True}
        
        @app.post("/bot{token}/getMyCommands")
        async def get_my_commands(token: str):
            """Get registered commands"""
            return {"ok": True, "result": self._commands_registered}
        
        @app.post("/bot{token}/getFile")
        async def get_file(token: str, request: Request):
            """Get file info (for media downloads)"""
            data = await request.json()
            file_id = data.get("file_id")
            return {
                "ok": True,
                "result": {
                    "file_id": file_id,
                    "file_unique_id": "unique_" + file_id,
                    "file_size": 1024,
                    "file_path": "documents/test.txt",
                }
            }
        
        @app.post("/bot{token}/sendChatAction")
        async def send_chat_action(token: str, request: Request):
            """Send typing/recording action"""
            return {"ok": True, "result": True}
        
        @app.get("/health")
        async def health():
            """Health check endpoint"""
            return {"status": "ok"}
    
    def _serialize_updates(self, updates: list[Update]) -> list[dict]:
        """Serialize updates for API response"""
        result = []
        for u in updates:
            update_dict = {"update_id": u.update_id}
            if u.message:
                update_dict["message"] = u.message
            if u.callback_query:
                update_dict["callback_query"] = u.callback_query
            if u.edited_message:
                update_dict["edited_message"] = u.edited_message
            if u.channel_post:
                update_dict["channel_post"] = u.channel_post
            result.append(update_dict)
        return result
    
    # --- Public API for tests ---
    
    def inject_message(self, text: str, chat_id: int = 123, 
                       from_username: str = "testuser",
                       is_group: bool = False) -> int:
        """Inject a message as if it came from a user"""
        update_id = self._next_update_id
        self._next_update_id += 1
        
        chat_type = "group" if is_group else "private"
        
        update = Update(
            update_id=update_id,
            message={
                "message_id": update_id + 100,
                "from": {
                    "id": chat_id,
                    "is_bot": False,
                    "first_name": from_username,
                    "username": from_username,
                },
                "chat": {
                    "id": chat_id,
                    "type": chat_type,
                },
                "date": 1234567890,
                "text": text,
            }
        )
        self._updates.append(update)
        logger.info(f"📥 Injected message: {text}")
        return update_id
    
    def inject_callback_query(self, data: str, chat_id: int = 123,
                              message_id: int = 100) -> int:
        """Inject a callback query (button press)"""
        update_id = self._next_update_id
        self._next_update_id += 1
        
        update = Update(
            update_id=update_id,
            callback_query={
                "id": f"cb_{update_id}",
                "from": {
                    "id": chat_id,
                    "is_bot": False,
                    "first_name": "testuser",
                    "username": "testuser",
                },
                "chat_instance": "123456789",
                "data": data,
                "message": {
                    "message_id": message_id,
                    "chat": {"id": chat_id, "type": "private"},
                    "text": "Some message with buttons",
                }
            }
        )
        self._updates.append(update)
        logger.info(f"📥 Injected callback query: {data}")
        return update_id
    
    def get_sent_messages(self) -> list[SentMessage]:
        """Get all messages sent by the bot"""
        return self._sent_messages.copy()
    
    def clear(self):
        """Clear all stored updates and messages"""
        self._updates.clear()
        self._sent_messages.clear()
        self._next_update_id = 1
    
    def get_last_message(self) -> SentMessage | None:
        """Get the most recent message sent by the bot"""
        return self._sent_messages[-1] if self._sent_messages else None
    
    def start_background(self):
        """Start the server in a background thread"""
        def run():
            uvicorn.run(self.app, host=self.host, port=self.port, log_level="warning")
        
        thread = Thread(target=run, daemon=True)
        thread.start()
        logger.info(f"🚀 Mock Telegram server started at http://{self.host}:{self.port}")
        return thread


# Convenience function for running as script
def main():
    server = MockTelegramServer()
    server.start_background()
    
    # Keep the main thread alive
    try:
        while True:
            asyncio.sleep(1)
    except KeyboardInterrupt:
        logger.info("Shutting down...")


if __name__ == "__main__":
    main()