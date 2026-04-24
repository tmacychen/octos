#!/usr/bin/env python3
"""
Telegram API Mock Server for Testing

This module provides a mock Telegram API server that simulates Telegram's API
for testing purposes. It allows testing the octos bot without needing real
Telegram API credentials.

Usage:
    # Start the mock server
    python -m bot_mock.mock_tg

    # Or import and use programmatically
    from bot_mock import MockTelegramServer
    server = MockTelegramServer()
    await server.start()
"""

import asyncio
import json
import logging
import os
import sys
import time
import uuid
from dataclasses import dataclass, field
from typing import Any
from pathlib import Path
from fastapi import FastAPI, Request, HTTPException
import uvicorn
from threading import Thread
import httpx

# Simple logging to stderr
logging.basicConfig(
    level=logging.WARNING,  # Reduce log noise for performance
    format='%(levelname)s:%(name)s:%(message)s',
    stream=sys.stderr,
    force=True
)
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
    message_id: int = None


# ══════════════════════════════════════════════════════════════════════════════
# Shared file for multi-worker uvicorn
# All workers append to the same JSONL file; /_all_messages reads it.
# ══════════════════════════════════════════════════════════════════════════════
import tempfile
_shared_msg_file = None


def _get_msg_file() -> str:
    global _shared_msg_file
    if _shared_msg_file is None:
        _shared_msg_file = tempfile.mktemp(suffix=".jsonl", prefix="mock_tg_")
    return _shared_msg_file


def create_app():
    """Factory function for uvicorn multi-worker mode.
    
    Each worker appends sent messages to a shared JSONL file.
    /_all_messages reads and aggregates all messages from that file.
    """
    import json
    import os
    import threading

    # Per-worker storage
    worker_updates: list = []
    worker_next_update_id = 1
    worker_edit_history: list = []
    _file_lock = threading.Lock()
    msg_file = _get_msg_file()
    
    app = FastAPI(title="Telegram Mock API")
    
    def serialize_updates(updates):
        result = []
        for u in updates:
            update_dict = {"update_id": u.update_id}
            if u.message:
                update_dict["message"] = u.message
            elif u.callback_query:
                update_dict["callback_query"] = u.callback_query
            result.append(update_dict)
        return result
    
    @app.get("/health")
    async def health():
        return {"status": "ok"}
    
    @app.get("/_sent_messages")
    async def get_sent_messages():
        """Return this worker's messages (reads from shared file)"""
        msgs = []
        if os.path.exists(msg_file):
            with _file_lock:
                try:
                    with open(msg_file) as f:
                        for line in f:
                            line = line.strip()
                            if line:
                                msgs.append(json.loads(line))
                except (json.JSONDecodeError, IOError):
                    pass
        return msgs
    
    @app.get("/_all_messages")
    async def get_all_messages():
        """Read all messages from the shared file."""
        msgs = []
        if os.path.exists(msg_file):
            with _file_lock:
                try:
                    with open(msg_file) as f:
                        for line in f:
                            line = line.strip()
                            if line:
                                msgs.append(json.loads(line))
                except (json.JSONDecodeError, IOError):
                    pass
        return msgs
    
    @app.post("/_clear")
    async def clear_state():
        # Clear both file and in-memory state
        with _file_lock:
            if os.path.exists(msg_file):
                os.remove(msg_file)
        worker_updates.clear()
        worker_edit_history.clear()
        return {"ok": True}
    
    @app.get("/_edit_history")
    async def get_edit_history():
        return worker_edit_history
    
    @app.api_route("/bot{token}/getUpdates", methods=["GET", "POST"])
    @app.api_route("/bot{token}/GetUpdates", methods=["GET", "POST"])
    async def get_updates(token: str, request: Request):
        try:
            data = await request.json()
        except Exception:
            data = {}
        offset = data.get("offset", 0)
        timeout = data.get("timeout", 30)
        
        updates = [u for u in worker_updates if u.update_id >= offset]
        
        if not updates and timeout > 0:
            waited = 0.0
            while waited < timeout:
                await asyncio.sleep(0.2)
                updates = [u for u in worker_updates if u.update_id >= offset]
                if updates:
                    break
                waited += 0.2
        
        return {"ok": True, "result": serialize_updates(updates)}
    
    @app.api_route("/bot{token}/sendMessage", methods=["GET", "POST"])
    @app.api_route("/bot{token}/SendMessage", methods=["GET", "POST"])
    async def send_message(token: str, request: Request):
        nonlocal worker_next_update_id
        try:
            data = await request.json()
        except Exception:
            data = {}
        text = data.get("text", "")
        chat_id = data.get("chat_id", 0)
        reply_to_message_id = data.get("reply_to_message_id")
        parse_mode = data.get("parse_mode")
        reply_markup = data.get("reply_markup")
        
        msg_id = worker_next_update_id
        worker_next_update_id += 1
        
        # Append to shared file (thread-safe + process-safe via file lock)
        entry = {"chat_id": chat_id, "text": text}
        with _file_lock:
            with open(msg_file, "a") as f:
                f.write(json.dumps(entry) + "\n")
        
        return {
            "ok": True,
            "result": {
                "message_id": msg_id,
                "chat": {"id": chat_id, "type": "private"},
                "text": text,
            }
        }
    
    @app.api_route("/bot{token}/editMessageText", methods=["GET", "POST"])
    async def edit_message_text(token: str, request: Request):
        try:
            data = await request.json()
        except Exception:
            data = {}
        text = data.get("text", "")
        chat_id = data.get("chat_id", 0)
        message_id = data.get("message_id", 0)
        
        worker_edit_history.append({
            "type": "editMessageText",
            "chat_id": chat_id,
            "message_id": message_id,
            "text": text,
        })
        
        # Update text in shared file (re-write)
        with _file_lock:
            if os.path.exists(msg_file):
                lines = []
                with open(msg_file) as f:
                    for line in f:
                        line = line.strip()
                        if line:
                            obj = json.loads(line)
                            if obj.get("message_id") == message_id:
                                obj["text"] = text
                            lines.append(json.dumps(obj) + "\n")
                with open(msg_file, "w") as f:
                    f.writelines(lines)
        
        return {"ok": True, "result": True}
    
    @app.api_route("/bot{token}/editMessageText", methods=["PATCH", "PUT"])
    async def edit_message_text_alt(token: str, request: Request):
        return await edit_message_text(token, request)
    
    @app.api_route("/bot{token}/getMe", methods=["GET", "POST"])
    async def get_me(token: str):
        return {
            "ok": True,
            "result": {
                "id": 123456789,
                "is_bot": True,
                "first_name": "TestBot",
                "username": "test_octos_bot",
            }
        }
    
    @app.post("/_inject")
    async def inject(request: Request):
        nonlocal worker_next_update_id
        data = await request.json()
        update_id = worker_next_update_id
        worker_next_update_id += 1
        
        update = {
            "update_id": update_id,
            "message": {
                "message_id": update_id,
                "from": {"id": data.get("chat_id", 123), "is_bot": False},
                "chat": {"id": data.get("chat_id", 123), "type": "private"},
                "text": data.get("text", ""),
                "date": 1234567890,
            }
        }
        worker_updates.append(type('Update', (), update)())
        
        return {"ok": True, "update_id": update_id}
    
    @app.post("/_inject_document")
    async def inject_document(request: Request):
        return {"ok": True, "update_id": worker_next_update_id}
    
    @app.post("/_inject_callback")
    async def inject_callback(request: Request):
        nonlocal worker_next_update_id
        data = await request.json()
        update_id = worker_next_update_id
        worker_next_update_id += 1
        
        update = {
            "update_id": update_id,
            "callback_query": {
                "id": f"cb_{update_id}",
                "from": {"id": data.get("chat_id", 123)},
                "message": {"message_id": data.get("message_id", 100), "chat": {"id": data.get("chat_id", 123)}},
                "data": data.get("data", ""),
            }
        }
        worker_updates.append(type('Update', (), update)())
        
        return {"ok": True, "update_id": update_id}
    
    @app.api_route("/bot{token}/{path:path}", methods=["GET", "POST"])
    async def catch_all(token: str, path: str, request: Request):
        return {"ok": True, "path": path}
    
    return app


class MockTelegramServer:
    """
    Mock Telegram API Server
    
    Simulates the Telegram Bot API for testing purposes.
    Implements the most common endpoints used by the octos bot.
    """
    
    def __init__(self, host: str = "127.0.0.1", port: int = 5000, 
                 max_message_length: int = 4096, setup_routes: bool = True):
        self.host = host
        self.port = port
        self.max_message_length = max_message_length  # Telegram limit: 4096 chars
        
        if setup_routes:
            self.app = FastAPI(title="Telegram Mock API")
            self._setup_routes()
        
        # Storage (only used in single-worker mode; multi-worker uses shared dict)
        self._updates: list = []
        self._sent_messages: list = []
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
        self._edit_history: list = []  # Record all edit operations for testing
        
        # Media directory for file uploads
        import tempfile
        self.media_dir = Path(tempfile.mkdtemp(prefix="mock_tg_media_"))
        
    def _setup_routes(self):
        """Set up FastAPI routes"""
        app = self.app
        
        # teloxide sends PascalCase paths, so register both cases for each endpoint

        @app.api_route("/bot{token}/getUpdates", methods=["GET", "POST"])
        @app.api_route("/bot{token}/GetUpdates", methods=["GET", "POST"])
        async def get_updates(token: str, request: Request):
            """Long polling endpoint - returns pending updates"""
            try:
                data = await request.json()
            except Exception:
                data = {}
            offset = int(data.get("offset", 0))
            timeout = int(data.get("timeout", 0))

            updates = [u for u in self._updates if u.update_id >= offset]

            if not updates and timeout > 0:
                # Poll up to timeout seconds, checking every 0.2s for new messages
                waited = 0.0
                while waited < timeout:
                    await asyncio.sleep(0.2)
                    waited += 0.2
                    updates = [u for u in self._updates if u.update_id >= offset]
                    if updates:
                        break

            return {"ok": True, "result": self._serialize_updates(updates)}
        
        @app.api_route("/bot{token}/sendMessage", methods=["GET", "POST"])
        @app.api_route("/bot{token}/SendMessage", methods=["GET", "POST"])
        async def send_message(token: str, request: Request):
            """Send a message - bot calls this to reply to users
            
            Messages exceeding max_message_length are automatically split into
            multiple SentMessage entries, mirroring what the real Telegram API
            expects and what the gateway's split_message() produces.
            """
            data = await request.json()
            
            chat_id = data.get("chat_id")
            text = data.get("text", "")
            parse_mode = data.get("parse_mode", "Markdown")
            reply_markup = data.get("reply_markup")
            reply_to_message_id = data.get("reply_to_message_id")
            
            if not chat_id:
                raise HTTPException(status_code=400, detail="chat_id is required")
            
            # Auto-split messages exceeding the length limit instead of rejecting.
            # The gateway already splits via split_message() (4000 chars), but
            # as a safety net we handle oversize messages gracefully here too.
            chunks = [text[i:i + self.max_message_length]
                      for i in range(0, len(text), self.max_message_length)]
            
            first_message_id = None
            for chunk_text in chunks:
                self._next_update_id += 1
                message_id = self._next_update_id + 1000
                self._sent_messages.append(SentMessage(
                    chat_id=chat_id,
                    text=chunk_text,
                    parse_mode=parse_mode,
                    reply_markup=reply_markup or {},
                    reply_to_message_id=reply_to_message_id,
                    message_id=message_id,
                ))
                if first_message_id is None:
                    first_message_id = message_id
                logger.debug(f"🤖 Bot sent message to {chat_id}: {chunk_text[:80]}...")
            
            return {
                "ok": True,
                "result": {
                    "message_id": first_message_id,
                    "from": self._bot_info,
                    "chat": {"id": chat_id, "type": "private"},
                    "text": text,
                    "date": 1234567890,
                }
            }
        
        @app.api_route("/bot{token}/sendDocument", methods=["GET", "POST"])
        @app.api_route("/bot{token}/SendDocument", methods=["GET", "POST"])
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
        
        @app.api_route("/bot{token}/sendVoice", methods=["GET", "POST"])
        @app.api_route("/bot{token}/SendVoice", methods=["GET", "POST"])
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
        
        @app.api_route("/bot{token}/sendAudio", methods=["GET", "POST"])
        @app.api_route("/bot{token}/SendAudio", methods=["GET", "POST"])
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
        
        @app.api_route("/bot{token}/editMessageText", methods=["GET", "POST"])
        @app.api_route("/bot{token}/EditMessageText", methods=["GET", "POST"])
        async def edit_message_text(token: str, request: Request):
            """Edit a message"""
            # Log raw request for debugging
            logger.debug(f"✏️ Edit request received - method={request.method}, url={request.url}")
            
            data = {}
            
            # Try to parse as JSON first (POST with JSON body)
            try:
                data = await request.json()
                logger.debug(f"✏️ Parsed as JSON: chat_id={data.get('chat_id')}, message_id={data.get('message_id')}")
            except Exception as e:
                logger.debug(f"✏️ Not JSON: {e}")
            
            # If no JSON, try form data (POST with form-urlencoded)
            if not data:
                try:
                    form_data = await request.form()
                    data = dict(form_data)
                    logger.debug(f"✏️ Parsed as form data: keys={list(data.keys())}")
                except Exception as e:
                    logger.debug(f"✏️ Not form data: {e}")
            
            # If still no data, try query parameters (GET request)
            if not data:
                data = dict(request.query_params)
                if data:
                    logger.debug(f"✏️ Parsed as query params: chat_id={data.get('chat_id')}, message_id={data.get('message_id')}")
            
            if not data:
                logger.error("✏️ No data found in request")
                raise HTTPException(status_code=400, detail="No data in request")
            
            chat_id = data.get("chat_id")
            message_id = data.get("message_id")
            text = data.get("text", "")
            
            # Convert types if needed (teloxide might send integers)
            if isinstance(chat_id, str):
                try:
                    chat_id = int(chat_id)
                except ValueError:
                    pass
            if isinstance(message_id, str):
                try:
                    message_id = int(message_id)
                except ValueError:
                    pass
            
            # Validate required fields
            if not chat_id or not message_id:
                logger.warning(f"✏️ Missing required fields: chat_id={chat_id}, message_id={message_id}")
                raise HTTPException(
                    status_code=400,
                    detail=f"Missing required fields: chat_id={chat_id}, message_id={message_id}"
                )
            
            # Record edit operation for testing
            import time
            edit_record = {
                "message_id": message_id,
                "chat_id": chat_id,
                "text": text,
                "timestamp": time.time(),
            }
            self._edit_history.append(edit_record)
            
            logger.debug(f"✏️ Message edited successfully: chat_id={chat_id}, message_id={message_id}")
            
            # Return a full Message object as per Telegram Bot API spec
            import time
            return {
                "ok": True,
                "result": {
                    "message_id": message_id,
                    "from": {
                        "id": 123456789,
                        "is_bot": True,
                        "first_name": "Octos Test Bot",
                        "username": "octos_test_bot"
                    },
                    "chat": {
                        "id": chat_id,
                        "type": "private"
                    },
                    "date": int(time.time()),
                    "text": text,
                }
            }
        
        @app.api_route("/bot{token}/deleteMessage", methods=["GET", "POST"])
        @app.api_route("/bot{token}/DeleteMessage", methods=["GET", "POST"])
        async def delete_message(token: str, request: Request):
            """Delete a message"""
            return {"ok": True, "result": True}
        
        @app.api_route("/bot{token}/answerCallbackQuery", methods=["GET", "POST"])
        @app.api_route("/bot{token}/AnswerCallbackQuery", methods=["GET", "POST"])
        async def answer_callback_query(token: str, request: Request):
            """Answer a callback query (dismiss loading spinner)"""
            return {"ok": True, "result": True}
        
        @app.api_route("/bot{token}/getMe", methods=["GET", "POST"])
        @app.api_route("/bot{token}/GetMe", methods=["GET", "POST"])
        async def get_me(token: str):
            """Get bot info"""
            return {"ok": True, "result": self._bot_info}

        @app.api_route("/bot{token}/getWebhookInfo", methods=["GET", "POST"])
        @app.api_route("/bot{token}/GetWebhookInfo", methods=["GET", "POST"])
        async def get_webhook_info(token: str):
            """Webhook info - return empty (we use long polling)"""
            return {"ok": True, "result": {
                "url": "",
                "has_custom_certificate": False,
                "pending_update_count": 0,
            }}

        @app.api_route("/bot{token}/setMyCommands", methods=["GET", "POST"])
        @app.api_route("/bot{token}/SetMyCommands", methods=["GET", "POST"])
        async def set_my_commands(token: str, request: Request):
            """Register bot commands"""
            try:
                data = await request.json()
            except Exception:
                data = {}
            self._commands_registered = data.get("commands", [])
            logger.debug(f"📝 Bot registered commands: {self._commands_registered}")
            return {"ok": True, "result": True}

        @app.api_route("/bot{token}/getMyCommands", methods=["GET", "POST"])
        @app.api_route("/bot{token}/GetMyCommands", methods=["GET", "POST"])
        async def get_my_commands(token: str):
            """Get registered commands"""
            return {"ok": True, "result": self._commands_registered}
        
        @app.api_route("/bot{token}/getFile", methods=["GET", "POST"])
        @app.api_route("/bot{token}/GetFile", methods=["GET", "POST"])
        async def get_file(token: str, request: Request):
            """Get file info (for media downloads)"""
            try:
                data = await request.json()
            except Exception:
                data = {}
            file_id = data.get("file_id", "unknown")
            return {
                "ok": True,
                "result": {
                    "file_id": file_id,
                    "file_unique_id": "unique_" + file_id,
                    "file_size": 1024,
                    "file_path": "documents/test.txt",
                }
            }
        
        @app.api_route("/bot{token}/sendChatAction", methods=["GET", "POST"])
        @app.api_route("/bot{token}/SendChatAction", methods=["GET", "POST"])
        async def send_chat_action(token: str, request: Request):
            """Send typing/recording action"""
            return {"ok": True, "result": True}
        
        @app.get("/health")
        async def health():
            """Health check endpoint"""
            return {"status": "ok"}

        # --- Test control endpoints (not part of real Telegram API) ---

        @app.post("/_inject")
        async def inject(request: Request):
            """Inject a message as if sent by a user (for test scripts)"""
            data = await request.json()
            update_id = self.inject_message(
                text=data.get("text", ""),
                chat_id=data.get("chat_id", 123),
                from_username=data.get("username", "testuser"),
                is_group=data.get("is_group", False),
            )
            return {"ok": True, "update_id": update_id}

        @app.post("/_inject_document")
        async def inject_document(request: Request):
            """Inject a document upload (for testing file handling)"""
            data = await request.json()
            file_path = data.get("file_path")
            caption = data.get("caption", "")
            
            if not file_path:
                raise HTTPException(status_code=400, detail="file_path is required")
            
            if not Path(file_path).exists():
                raise HTTPException(status_code=404, detail=f"File not found: {file_path}")
            
            update_id = self.inject_document(
                file_path=file_path,
                caption=caption,
                chat_id=data.get("chat_id", 123),
                from_username=data.get("username", "testuser"),
            )
            return {"ok": True, "update_id": update_id}

        @app.post("/_inject_callback")
        async def inject_callback(request: Request):
            """Inject a callback query (button press) for test scripts"""
            data = await request.json()
            update_id = self.inject_callback_query(
                data=data.get("data", ""),
                chat_id=data.get("chat_id", 123),
                message_id=data.get("message_id", 100),
            )
            return {"ok": True, "update_id": update_id}

        @app.get("/_sent_messages")
        async def get_sent_messages():
            """Return all messages sent by the bot (for test assertions)"""
            return [
                {
                    "chat_id": m.chat_id,
                    "text": m.text,
                    "parse_mode": m.parse_mode,
                    "reply_to_message_id": m.reply_to_message_id,
                    "message_id": m.message_id,
                }
                for m in self._sent_messages
            ]

        @app.post("/_clear")
        async def clear_state():
            """Clear stored messages only (not update_id counter)"""
            self._updates.clear()
            self._sent_messages.clear()
            self._edit_history.clear()
            return {"ok": True}

        @app.get("/_edit_history")
        async def get_edit_history():
            """Return all message edit operations (for stream edit tests)"""
            return self._edit_history.copy()

        @app.api_route("/bot{token}/{path:path}", methods=["GET", "POST"])
        async def catch_all(token: str, path: str, request: Request):
            """Catch-all: log unhandled API calls for debugging"""
            try:
                body = await request.json()
            except Exception:
                body = {}
            logger.warning(f"⚠️  Unhandled API call: {request.method} /{path} body={body}")
            return {"ok": True, "result": True}
    
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
        # Truncate long messages in logs to avoid output explosion
        text_preview = text[:100] + "..." if len(text) > 100 else text
        logger.debug(f"💬 Injected message ({len(text)} bytes): {text_preview}")
        return update_id
    
    def inject_document(self, file_path: str, caption: str, chat_id: int = 123,
                       from_username: str = "testuser") -> int:
        """Inject a document upload as if sent by a user
        
        This simulates Telegram's file upload mechanism.
        The file will be copied to media_dir and the path passed to octos.
        """
        import shutil
        
        update_id = self._next_update_id
        self._next_update_id += 1
        
        # Copy file to media directory (simulating Telegram download)
        filename = Path(file_path).name
        dest_path = self.media_dir / f"injected_{update_id}_{filename}"
        shutil.copy2(file_path, dest_path)
        
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
                    "type": "private",
                },
                "date": 1234567890,
                "caption": caption,
                "document": {
                    "file_id": f"doc_{update_id}",
                    "file_name": filename,
                    "mime_type": "application/octet-stream",
                    "file_size": dest_path.stat().st_size,
                },
            }
        )
        self._updates.append(update)
        file_size_mb = dest_path.stat().st_size / (1024 * 1024)
        logger.debug(f"📎 Injected document: {filename} ({file_size_mb:.1f}MB) → {dest_path}")
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
        logger.debug(f"🎯 Injected callback query: {data}")
        return update_id
    
    def get_sent_messages(self) -> list:
        """Get all messages sent by the bot"""
        return [{"chat_id": m.chat_id, "text": m.text} for m in self._sent_messages]
    
    def clear(self):
        """Clear all stored updates and messages"""
        self._updates.clear()
        self._sent_messages.clear()
        self._edit_history.clear()
    
    def get_edit_history(self) -> list:
        resp = httpx.get(f"http://{self.host}:{self.port}/_edit_history", timeout=5)
        return resp.json()
    
    def clear_edit_history(self):
        """Clear edit history"""
        self._edit_history.clear()
    
    def get_last_message(self) -> dict | None:
        """Get the most recent message sent by the bot (multi-worker aware)"""
        msgs = self.get_sent_messages()
        return msgs[-1] if msgs else None
    
    def start_background(self, log_file=None):
        """Start the server in a background thread"""
        if log_file:
            from pathlib import Path
            log_path = Path(log_file)
            log_path.parent.mkdir(parents=True, exist_ok=True)
            
            # Configure logging to both file and stdout
            formatter = logging.Formatter('%(asctime)s - %(name)s - %(levelname)s - %(message)s')
            
            # File handler
            file_handler = logging.FileHandler(log_file, mode='a')
            file_handler.setFormatter(formatter)
            file_handler.setLevel(logging.INFO)
            
            # Stdout handler (for subprocess pipe capture by test_run.py)
            stdout_handler = logging.StreamHandler(sys.stdout)
            stdout_handler.setFormatter(formatter)
            stdout_handler.setLevel(logging.INFO)
            
            # Configure root logger
            root_logger = logging.getLogger()
            root_logger.addHandler(file_handler)
            root_logger.addHandler(stdout_handler)
            root_logger.setLevel(logging.INFO)
            
            # Configure uvicorn loggers
            uvicorn_logger = logging.getLogger("uvicorn")
            uvicorn_logger.addHandler(file_handler)
            uvicorn_logger.addHandler(stdout_handler)
            uvicorn_logger.setLevel(logging.INFO)
            
            uvicorn_access_logger = logging.getLogger("uvicorn.access")
            uvicorn_access_logger.addHandler(file_handler)
            uvicorn_access_logger.addHandler(stdout_handler)
            uvicorn_access_logger.setLevel(logging.INFO)
            
            uvicorn_error_logger = logging.getLogger("uvicorn.error")
            uvicorn_error_logger.addHandler(file_handler)
            uvicorn_error_logger.addHandler(stdout_handler)
            uvicorn_error_logger.setLevel(logging.INFO)
        
        def run():
            # Use simple log_level parameter for stability
            uvicorn.run(
                self.app,
                host=self.host,
                port=self.port,
                log_level="warning",  # Reduce noise
                timeout_keep_alive=60,
                limit_concurrency=100,
            )
        
        thread = Thread(target=run, daemon=True)  # Keep as daemon thread, but with better error handling
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