#!/usr/bin/env python3
"""
Discord API Mock Server for Testing

Simulates the complete Discord Gateway (WebSocket) + REST (HTTP) surface
so that serenity-based bots can connect and exchange messages without real
Discord credentials.

Architecture
~~~~~~~~~~~
::

  octos bot (serenity)
    │
    ├── GET /api/v10/gateway/bot ──► returns ws://127.0.0.1:{ws_port}/?v=10&encoding=json
    │                                  (this FastAPI app on {port})
    │
    ├── WS / (Gateway WebSocket) ◄─── Hello → Identify → Ready + MESSAGE_CREATE dispatch
    │
    ├── POST /api/v10/channels/{id}/messages  (send / edit)
    │
    ├── PUT   /api/v10/channels/{id}/messages/{mid} (edit_message_text)
    │
    ├── DELETE /api/v10/channels/{id}/messages/{mid}
    │
    ├── POST /api/v10/channels/{id}/messages/{mid}/reactions/{emoji}/@me
    │
    ├── DELETE /api/v10/channels/{id}/messages/{mid}/reactions/{emoji}/@me
    │
    └── GET /api/v10/users/@me

Control endpoints (not part of Discord API):
  POST /_inject          inject a user message → dispatches as MESSAGE_CREATE via WS
  POST /_inject_interaction  inject an interaction (slash command, component)
  GET  /_sent_messages   return all messages sent by the bot
  POST /_clear           clear stored messages & updates
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
import uuid
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from fastapi import FastAPI, Request, HTTPException, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse
import uvicorn
from threading import Thread

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------

DEFAULT_GUILD_ID = "927930120308613120"
DEFAULT_CHANNEL_ID = "1039178386623557754"
DEFAULT_USER_ID = "123456789012345678"


@dataclass
class SentMessage:
    """A message sent by the bot via REST API."""
    channel_id: str
    content: str
    message_id: str = ""
    embeds: list = field(default_factory=list)


@dataclass
class InjectedMessage:
    """A message injected by test scripts to be dispatched as MESSAGE_CREATE."""
    content: str
    channel_id: str = DEFAULT_CHANNEL_ID
    guild_id: Optional[str] = DEFAULT_GUILD_ID  # None = DM
    sender_id: str = DEFAULT_USER_ID
    sender_name: str = "TestUser"
    mention_everyone: bool = False


@dataclass
class InjectedInteraction:
    """An interaction injected by test scripts (slash command or component)."""
    data: dict  # interaction data payload
    channel_id: str = DEFAULT_CHANNEL_ID
    guild_id: Optional[str] = DEFAULT_GUILD_ID
    sender_id: str = DEFAULT_USER_ID
    token: str = ""
    message_id: Optional[str] = None


@dataclass
class InjectedDocument:
    """A document/file upload injected by test scripts."""
    file_path: str
    caption: str = ""
    channel_id: str = DEFAULT_CHANNEL_ID
    guild_id: Optional[str] = DEFAULT_GUILD_ID
    sender_id: str = DEFAULT_USER_ID
    sender_name: str = "TestUser"


# ---------------------------------------------------------------------------
# Mock server
# ---------------------------------------------------------------------------

class MockDiscordServer:
    """
    Mock Discord API Server.

    Provides both:
    - **REST HTTP** endpoints that serenity's Http client calls
    - **Gateway WebSocket** endpoint that serenity's Client connects to for events
    """

    def __init__(self, host: str = "127.0.0.1", port: int = 5001):
        self.host = host
        self.port = port
        self.app = FastAPI(title="Discord Mock API")
        self._setup_routes()

        # State
        self._sent_messages: List[SentMessage] = []
        self._injected_messages: List[InjectedMessage] = []
        self._injected_interactions: List[InjectedInteraction] = []
        self._injected_documents: List[InjectedDocument] = []
        self._message_counter: int = int(time.time() * 1000)  # snowflake-like
        self._next_update_id: int = 1

        # Bot info returned by GET /users/@me
        self._bot_info = {
            "id": "987654321098765432",
            "username": "TestBot",
            "discriminator": "0001",
            "global_name": "TestBot",
            "avatar": None,
            "bot": True,
            "mfa_enabled": False,
            "locale": "en-US",
            "verified": True,
            "email": None,
            "flags": 0,
            "premium_type": 0,
            "public_flags": 0,
        }

        # Active WebSocket connections (gateway sessions)
        self._ws_clients: List[WebSocket] = []

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _next_id(self) -> int:
        self._message_counter += 1
        return self._message_counter

    def _setup_routes(self):
        app = self.app

        # ====== Discord REST API endpoints ======

        @app.get("/api/v10/gateway")
        async def get_gateway():
            """Return gateway URL (simpler version without bot-specific info)."""
            logger.info("=== GATEWAY CALLED === returning ws URL")
            return {"url": f"ws://{self.host}:{self.port}"}

        @app.get("/api/v10/gateway/bot")
        async def get_gateway_bot():
            """Return gateway URL pointing to our own mock websocket."""
            logger.info("=== GATEWAY/BOT CALLED === returning ws URL")
            return {
                "url": f"ws://{self.host}:{self.port}",
                "shards": 1,
                "session_start_limit": {
                    "total": 1000,
                    "remaining": 999,
                    "reset_after": 0,
                    "max_concurrency": 1,
                },
                "_trace": ["mock-gateway"],
            }

        @app.get("/api/v10/users/@me")
        async def get_current_user():
            """Return mock bot user info."""
            return JSONResponse(self._bot_info)

        @app.post("/api/v10/channels/{channel_id}/messages")
        async def create_message(channel_id: str, request: Request):
            """Send a message to a channel (called by bot)."""
            body = await request.json()
            msg_id = str(self._next_id())

            content = body.get("content", "")
            # Handle empty content with embeds
            if not content:
                embeds = body.get("embeds", [])
                if embeds:
                    # Extract embed title+description as fallback text
                    parts = []
                    for emb in embeds:
                        if emb.get("title"):
                            parts.append(f"**{emb['title']}**")
                        if emb.get("description"):
                            parts.append(emb["description"])
                    content = "\n".join(parts) if parts else "[embed]"
                else:
                    content = ""

            sent = SentMessage(
                channel_id=channel_id,
                content=content,
                message_id=msg_id,
                embeds=body.get("embeds", []),
            )
            self._sent_messages.append(sent)

            logger.info(f"🤖 Bot sent message to {channel_id}: {content[:80]}")

            resp: Dict[str, Any] = {
                "id": msg_id,
                "channel_id": channel_id,
                "guild_id": body.get("guild_id"),
                "author": {
                    "id": self._bot_info["id"],
                    "username": self._bot_info["username"],
                    "discriminator": self._bot_info["discriminator"],
                    "global_name": self._bot_info["global_name"],
                    "avatar": None,
                    "bot": True,
                },
                "content": content,
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00"),
                "edited_timestamp": None,
                "tts": False,
                "mention_everyone": False,
                "mention_roles": [],
                "attachments": [],
                "embeds": body.get("embeds", []),
                "pinned": False,
                "type": 0,
            }
            return JSONResponse(resp)

        @app.put("/api/v10/channels/{channel_id}/messages/{message_id}")
        async def edit_message(channel_id: str, message_id: str, request: Request):
            """Edit an existing message."""
            body = await request.json()
            content = body.get("content", "")
            logger.info(f"✏️ Bot edited message {message_id} in {channel_id}: {content[:80]}")

            # Track as sent message with [edited] prefix
            self._sent_messages.append(SentMessage(
                channel_id=channel_id,
                content=f"[edited] {content}",
                message_id=message_id,
            ))

            return JSONResponse({
                "id": message_id,
                "channel_id": channel_id,
                "content": content,
                "edited_timestamp": time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00"),
            })

        @app.delete("/api/v10/channels/{channel_id}/messages/{message_id}")
        async def delete_message(channel_id: str, message_id: str):
            """Delete a message."""
            logger.info(f"🗑️ Bot deleted message {message_id} in {channel_id}")
            return JSONResponse({})

        @app.put("/api/v10/channels/{channel_id}/messages/{message_id}/reactions/{emoji:path}/@me")
        async def create_reaction(channel_id: str, message_id: str, emoji: str):
            """Add reaction to a message."""
            logger.info(f"👍 Bot reacted {emoji} to msg {message_id}")
            return JSONResponse({})

        @app.delete("/api/v10/channels/{channel_id}/messages/{message_id}/reactions/{emoji:path}/@me")
        async def delete_reaction(channel_id: str, message_id: str, emoji: str):
            """Remove bot's reaction from a message."""
            logger.info(f"👎 Bot removed reaction {emoji} from msg {message_id}")
            return JSONResponse({})

        @app.post("/api/v10/interactions/{interaction_id}/{token}/callback")
        async def interaction_callback(interaction_id: str, token: str, request: Request):
            """Respond to an interaction (serenity calls this after receiving one)."""
            body = await request.json()
            logger.info(f"Interaction callback: type={body.get('type')}, id={interaction_id}")
            # Return a simple ACK response for the interaction
            return JSONResponse({
                "id": interaction_id,
                "type": body.get("type", 4),  # 4 = CHANNEL_MESSAGE_WITH_SOURCE
                "data": body.get("data", {}),
            })

        @app.post("/api/v10/channels/{channel_id}/typing")
        async def trigger_typing(channel_id: str):
            """Trigger typing indicator - just ack."""
            return JSONResponse({})

        # ====== Gateway WebSocket endpoint ======

        @app.websocket("/")
        async def gateway_websocket(websocket: WebSocket):
            """
            Discord Gateway protocol handler.

            Implements:
              OP 10 HELLO          → send heartbeat interval
              OP 2 IDENTIFY        → receive auth, send READY
              OP 1 HEARTBEAT       → respond with HEARTBEAT ACK
              OP 3 PRESENCE_UPDATE → ignore (or store)
              OP 8 REQUEST_MEMBERS → ignore
            """
            logger.info("=== WS CLIENT CONNECTING ===")
            await websocket.accept()
            logger.info("=== WS CLIENT ACCEPTED ===")
            self._ws_clients.append(websocket)
            session_id = f"mock-session-{uuid.uuid4().hex[:12]}"
            seq: int = 0

            try:
                # Send OP 10 HELLO
                hello_payload = {
                    "op": 10,
                    "d": {
                        "heartbeat_interval": 45000,
                        "_trace": ["mock-gateway"],
                    },
                }
                await websocket.send_json(hello_payload)
                logger.info("WS: Sent HELLO")

                # Main loop: handle incoming opcodes
                while True:
                    try:
                        raw = await asyncio.wait_for(
                            websocket.receive_text(), timeout=60.0
                        )
                    except Exception:
                        break

                    try:
                        data = json.loads(raw)
                    except json.JSONDecodeError:
                        continue

                    opcode = data.get("op")
                    d = data.get("d", {})

                    if opcode == 2:  # IDENTIFY
                        logger.info("WS: Received IDENTIFY, sending READY")

                        ready_payload = {
                            "op": 0,  # DISPATCH
                            "t": "READY",
                            "s": seq,
                            "d": {
                                "v": 10,
                                "user": self._bot_info,
                                "session_id": session_id,
                                "resume_gateway_url": f"ws://{self.host}:{self.port}",
                                "session_type": "normal",
                                "application": {
                                    "id": self._bot_info["id"],
                                    "flags": 0,
                                },
                                "guilds": [
                                    {
                                        "id": DEFAULT_GUILD_ID,
                                        "unavailable": False,
                                        "joined_at": "2024-01-01T00:00:00+00:00",
                                    }
                                ],
                                "private_channels": [],
                                "relationships": [],
                                "user_settings": {},
                                "_trace": ["mock-gateway"],
                            },
                        }
                        await websocket.send_json(ready_payload)
                        seq += 1

                        # Dispatch any pending injected interactions now that we're ready
                        for inter in list(self._injected_interactions):
                            seq = await self._dispatch_interaction(
                                websocket, inter, seq
                            )
                            self._injected_interactions.remove(inter)

                    elif opcode == 1:  # HEARTBEAT
                        ack = {"op": 11, "d": d}  # HEARTBEAT_ACK (null is also valid)
                        await websocket.send_json(ack)

                    elif opcode == 3:  # PRESENCE_UPDATE
                        pass  # ignore

                    elif opcode == 8:  # REQUEST_GUILD_MEMBERS
                        pass  # ignore

                    else:
                        logger.debug(f"WS: Unhandled opcode {opcode}")

            except WebSocketDisconnect:
                logger.info("WS: Client disconnected")
            except Exception as e:
                logger.error(f"WS error: {e}")
            finally:
                if websocket in self._ws_clients:
                    self._ws_clients.remove(websocket)

        # ====== Test control endpoints ======
        app.add_api_route("/_inject", self._handle_inject, methods=["POST"])
        app.add_api_route("/_inject_interaction", self._handle_inject_interaction, methods=["POST"])
        app.add_api_route("/_inject_document", self._handle_inject_document, methods=["POST"])
        app.add_api_route("/_sent_messages", self._handle_sent_messages, methods=["GET"])
        app.add_api_route("/_clear", self._handle_clear, methods=["POST"])
        app.add_api_route("/health", self._handle_health, methods=["GET"])

    # ------------------------------------------------------------------
    # Message dispatch helpers
    # ------------------------------------------------------------------

    async def _dispatch_injected_messages(self, websocket: WebSocket, start_seq: int) -> int:
        """Dispatch all queued injected messages as MESSAGE_CREATE events."""
        seq = start_seq
        for msg in self._injected_messages.copy():
            event = self._build_message_create(msg)
            payload = {
                "op": 0,  # DISPATCH
                "t": "MESSAGE_CREATE",
                "s": seq,
                "d": event,
            }
            await websocket.send_json(payload)
            logger.info(f"🔄 Dispatched MESSAGE_CREATE ({len(msg.content)} bytes): {msg.content[:50]}")
            seq += 1
            self._injected_messages.remove(msg)
        return seq

    async def _dispatch_interaction(self, websocket: WebSocket, inter: InjectedInteraction, seq: int) -> int:
        """Dispatch an injected interaction as INTERACTION_CREATE event."""
        interaction_data = {
            "id": str(uuid.uuid4()),
            "application_id": self._bot_info["id"],
            "type": 2 if inter.message_id else 1,  # 1=ping, 2=APPLICATION_COMMAND, 3=MESSAGE_COMPONENT
            "data": inter.data,
            "token": inter.token or f"mock-interaction-token-{uuid.uuid4().hex[:16]}",
            "version": 1,
            "channel": {
                "id": inter.channel_id,
                "type": 0,  # GUILD_TEXT
            },
            "channel_id": inter.channel_id,
            "guild_id": inter.guild_id,
            "member": {
                "user": {
                    "id": inter.sender_id,
                    "username": "TestUser",
                    "discriminator": "0001",
                    "global_name": "TestUser",
                    "avatar": None,
                    "bot": False,
                },
                "roles": [],
                "joined_at": "2024-01-01T00:00:00+00:00",
                "deaf": False,
                "mute": False,
                "pending": False,
            },
            "user": {
                "id": inter.sender_id,
                "username": "TestUser",
                "discriminator": "0001",
                "global_name": "TestUser",
                "avatar": None,
                "bot": False,
            },
        }
        if inter.message_id:
            interaction_data["message"] = {
                "id": inter.message_id,
                "channel_id": inter.channel_id,
                "content": "",
                "author": self._bot_info,
            }

        payload = {
            "op": 0,  # DISPATCH
            "t": "INTERACTION_CREATE",
            "s": seq,
            "d": interaction_data,
        }
        await websocket.send_json(payload)
        logger.info(f"Dispatched INTERACTION_CREATE: {list(inter.data.keys())}")
        return seq + 1

    def _build_message_create(self, msg: InjectedMessage) -> dict:
        """Build a MESSAGE_CREATE event payload matching Discord's format."""
        mid = str(self._next_id())
        base: Dict[str, Any] = {
            "id": mid,
            "channel_id": msg.channel_id,
            "author": {
                "id": msg.sender_id,
                "username": msg.sender_name,
                "discriminator": "0001",
                "global_name": msg.sender_name,
                "avatar": None,
                "bot": False,
            },
            "content": msg.content,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00"),
            "edited_timestamp": None,
            "tts": False,
            "mention_everyone": msg.mention_everyone,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": False,
            "type": 0,
            "flags": 0,
            "referenced_message": None,
        }

        if msg.guild_id is None:
            # DM channel
            base["guild_id"] = None
            base["member"] = None
        else:
            base["guild_id"] = msg.guild_id
            base["member"] = {
                "roles": [],
                "joined_at": "2024-01-01T00:00:00+00:00",
                "deaf": False,
                "mute": False,
            }

        return base

    # ------------------------------------------------------------------
    # Test control endpoint handlers
    # ------------------------------------------------------------------

    async def _handle_inject(self, request: Request):
        """Inject a user message, dispatch as MESSAGE_CREATE to WS clients."""
        data = await request.json()
        msg = InjectedMessage(
            content=data.get("text", ""),
            channel_id=str(data.get("channel_id", data.get("chat_id", DEFAULT_CHANNEL_ID))),
            guild_id=str(data.get("guild_id")) if data.get("guild_id") else None,
            sender_id=str(data.get("sender_id", DEFAULT_USER_ID)),
            sender_name=str(data.get("username", "TestUser")),
            mention_everyone=data.get("mention_everyone", False),
        )
        self._injected_messages.append(msg)
        for ws in list(self._ws_clients):
            try:
                await self._dispatch_injected_messages(ws, 0)
            except Exception as e:
                logger.warning(f"Failed to dispatch to WS client: {e}")
        return {"ok": True}

    async def _handle_inject_interaction(self, request: Request):
        """Inject an interaction, dispatch as INTERACTION_CREATE to WS clients."""
        data = await request.json()
        inter = InjectedInteraction(
            data=data.get("data", {}),
            channel_id=str(data.get("channel_id", data.get("chat_id", DEFAULT_CHANNEL_ID))),
            guild_id=str(data.get("guild_id")) if data.get("guild_id") else None,
            sender_id=str(data.get("sender_id", DEFAULT_USER_ID)),
            token=data.get("token", ""),
            message_id=data.get("message_id"),
        )
        self._injected_interactions.append(inter)
        for ws in list(self._ws_clients):
            try:
                for i in list(self._injected_interactions):
                    await self._dispatch_interaction(ws, i, 0)
                    self._injected_interactions.remove(i)
            except Exception as e:
                logger.warning(f"Failed to dispatch interaction: {e}")
        return {"ok": True}

    async def _handle_inject_document(self, request: Request):
        """Inject a document/file upload (for testing file handling)"""
        from pathlib import Path
        
        data = await request.json()
        file_path = data.get("file_path")
        caption = data.get("caption", "")
        
        if not file_path:
            raise HTTPException(status_code=400, detail="file_path is required")
        
        if not Path(file_path).exists():
            raise HTTPException(status_code=404, detail=f"File not found: {file_path}")
        
        doc = InjectedDocument(
            file_path=file_path,
            caption=caption,
            channel_id=str(data.get("channel_id", DEFAULT_CHANNEL_ID)),
            guild_id=str(data.get("guild_id")) if data.get("guild_id") else None,
            sender_id=str(data.get("sender_id", DEFAULT_USER_ID)),
            sender_name=str(data.get("username", "TestUser")),
        )
        self._injected_documents.append(doc)
        
        # Dispatch as MESSAGE_CREATE with attachment
        for ws in list(self._ws_clients):
            try:
                await self._dispatch_injected_messages(ws, 0)
            except Exception as e:
                logger.warning(f"Failed to dispatch document to WS client: {e}")
        
        return {"ok": True}

    async def _handle_sent_messages(self):
        """Return all messages sent by the bot."""
        return [
            {
                "chat_id": m.channel_id,  # Alias for compatibility with BaseMockRunner
                "channel_id": m.channel_id,
                "text": m.content,
                "message_id": m.message_id,
                "embeds": m.embeds,
            }
            for m in self._sent_messages
        ]

    async def _handle_clear(self):
        """Clear stored state."""
        self._sent_messages.clear()
        self._injected_messages.clear()
        self._injected_interactions.clear()
        self._injected_documents.clear()
        return {"ok": True}

    async def _handle_health(self):
        """Health check."""
        return {"status": "ok", "ws_clients": len(self._ws_clients)}

    # ---- Public programmatic API ----

    def inject_message(
        self,
        text: str,
        channel_id: str = DEFAULT_CHANNEL_ID,
        sender_id: str = DEFAULT_USER_ID,
        username: str = "TestUser",
        guild_id: Optional[str] = DEFAULT_GUILD_ID,
    ) -> int:
        """Inject a message programmatically. Returns internal ID."""
        msg = InjectedMessage(
            content=text,
            channel_id=channel_id,
            sender_id=sender_id,
            sender_name=username,
            guild_id=guild_id,
        )
        self._injected_messages.append(msg)
        update_id = self._next_update_id
        self._next_update_id += 1
        logger.info(f"💬 Injected message ({len(text)} bytes): {text[:50]}")
        return update_id

    def inject_interaction_data(
        self,
        data: dict,
        channel_id: str = DEFAULT_CHANNEL_ID,
        sender_id: str = DEFAULT_USER_ID,
        message_id: Optional[str] = None,
    ) -> int:
        """Inject an interaction programmatically. Returns internal ID."""
        inter = InjectedInteraction(
            data=data,
            channel_id=channel_id,
            sender_id=sender_id,
            message_id=message_id,
        )
        self._injected_interactions.append(inter)
        update_id = self._next_update_id
        self._next_update_id += 1
        logger.info(f"🎯 Injected interaction: {data.get('name', 'unknown')}")
        return update_id

    def get_sent_messages(self) -> List[SentMessage]:
        """Get all messages sent by the bot."""
        return self._sent_messages.copy()

    def clear(self):
        """Clear all stored state."""
        self._sent_messages.clear()
        self._injected_messages.clear()
        self._injected_interactions.clear()

    def get_last_message(self) -> Optional[SentMessage]:
        """Get most recent sent message."""
        return self._sent_messages[-1] if self._sent_messages else None

    def start_background(self):
        """Start the FastAPI server in a background thread."""
        import threading

        def run():
            uvicorn.run(self.app, host=self.host, port=self.port, log_level="warning")

        thread = Thread(target=run, daemon=True)
        thread.start()
        logger.info(f"🚀 Mock Discord server started at http://{self.host}:{self.port} (WS at same address)")
        return thread


def main():
    server = MockDiscordServer()
    server.start_background()

    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        logger.info("Shutting down...")


if __name__ == "__main__":
    main()
