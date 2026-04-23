#!/usr/bin/env python3
"""
BaseMockRunner - Mock Server 测试辅助工具的基类。

提供与 Mock Server 交互的通用方法：
  - get_sent_messages: 获取 bot 已发送的所有消息
  - wait_for_reply: 等待 bot 发送新消息
  - health: 检查 Mock Server 是否在线
"""

import time
from typing import Optional

import httpx


class BaseMockRunner:
    """连接已运行的 Mock Server，提供测试辅助方法的基类。"""

    def __init__(self, base_url: str):
        self.base_url = base_url

    def get_sent_messages(self, timeout: int = 10) -> list[dict]:
        """获取 bot 已发送的所有消息
        
        Args:
            timeout: HTTP 请求超时时间（秒），默认 10s
        """
        resp = httpx.get(f"{self.base_url}/_sent_messages", timeout=timeout)
        resp.raise_for_status()
        return resp.json()

    def wait_for_reply(self, count_before: int = 0,
                       timeout: int = 10, poll_interval: float = 0.5,
                       chat_id: Optional[int] = None) -> Optional[dict]:
        """
        等待 bot 发送新消息，返回最新一条。
        count_before: 调用前已有的消息数量
        timeout: 最长等待秒数
        poll_interval: 轮询间隔秒数（默认 0.5s，比之前更快响应）
        chat_id: 可选，只等待特定 chat_id 的消息（并发测试必需）
        """
        # 🔥 OPTIMIZATION: Check immediately before waiting
        # This avoids unnecessary delay if message is already available
        msgs = self.get_sent_messages()
        if len(msgs) > count_before:
            if chat_id is not None:
                for msg in reversed(msgs[count_before:]):
                    if msg.get("chat_id") == chat_id or msg.get("channel_id") == chat_id:
                        return msg
            else:
                return msgs[-1]
        
        # Enter polling loop if message not yet available
        elapsed = 0.0
        while elapsed < timeout:
            time.sleep(poll_interval)
            elapsed += poll_interval
            msgs = self.get_sent_messages()
            if len(msgs) > count_before:
                # If chat_id specified, filter messages for this chat
                if chat_id is not None:
                    for msg in reversed(msgs[count_before:]):
                        if msg.get("chat_id") == chat_id or msg.get("channel_id") == chat_id:
                            return msg
                else:
                    return msgs[-1]
        return None

    def health(self) -> bool:
        """检查 Mock Server 是否在线"""
        try:
            resp = httpx.get(f"{self.base_url}/health", timeout=2)
            return resp.status_code == 200
        except Exception:
            return False

    def clear(self) -> None:
        """清除 Mock Server 中存储的所有消息和状态"""
        resp = httpx.post(f"{self.base_url}/_clear", timeout=5)
        resp.raise_for_status()
