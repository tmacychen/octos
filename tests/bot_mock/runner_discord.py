#!/usr/bin/env python3
"""
DiscordTestRunner - 连接已运行的 Discord Mock Server，供 pytest 测试用例使用。
"""

from typing import Optional

import httpx

from base_runner import BaseMockRunner

MOCK_BASE_URL = "http://127.0.0.1:5001"


class DiscordTestRunner(BaseMockRunner):
    """Discord Mock Server 测试辅助工具。"""

    def __init__(self, base_url: str = MOCK_BASE_URL):
        super().__init__(base_url)

    def inject(
        self,
        text: str,
        channel_id: str = "1039178386623557754",
        sender_id: str = "123456789012345678",
        username: str = "TestUser",
        guild_id: Optional[str] = "927930120308613120",
    ) -> dict:
        """向 Mock Server 注入一条用户消息（分发为 MESSAGE_CREATE）"""
        payload: dict = {
            "text": text,
            "channel_id": channel_id,
            "sender_id": sender_id,
            "username": username,
        }
        if guild_id:
            payload["guild_id"] = guild_id
        resp = httpx.post(f"{self.base_url}/_inject", json=payload, timeout=5)
        resp.raise_for_status()
        return resp.json()
