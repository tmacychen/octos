#!/usr/bin/env python3
"""
BotTestRunner - 连接已运行的 Mock Server，供 pytest 测试用例使用。

Mock Server 和 Bot 进程由 run_test.fish 负责启动，
pytest 只需通过 HTTP 与 Mock Server 交互。
"""

import asyncio
import os
import time
from pathlib import Path
from typing import Callable, Any

import httpx

MOCK_BASE_URL = os.environ.get("MOCK_BASE_URL", "http://127.0.0.1:5000")


class BotTestRunner:
    """
    连接已运行的 Mock Server，提供测试辅助方法。

    用法：
        runner = BotTestRunner()

        # 注入消息
        runner.inject("/start")

        # 等待并断言回复
        msg = runner.wait_for_reply(timeout=10)
        assert msg is not None

        # 清空状态（每个测试前调用）
        runner.clear()
    """

    def __init__(self, base_url: str = MOCK_BASE_URL):
        self.base_url = base_url

    def inject(self, text: str, chat_id: int = 123,
               username: str = "testuser", is_group: bool = False) -> dict:
        """向 Mock Server 注入一条用户消息"""
        resp = httpx.post(f"{self.base_url}/_inject", json={
            "text": text,
            "chat_id": chat_id,
            "username": username,
            "is_group": is_group,
        })
        resp.raise_for_status()
        return resp.json()

    def inject_callback(self, data: str, chat_id: int = 123,
                        message_id: int = 100) -> dict:
        """注入一个按钮回调"""
        resp = httpx.post(f"{self.base_url}/_inject_callback", json={
            "data": data,
            "chat_id": chat_id,
            "message_id": message_id,
        })
        resp.raise_for_status()
        return resp.json()

    def get_sent_messages(self) -> list[dict]:
        """获取 bot 已发送的所有消息"""
        resp = httpx.get(f"{self.base_url}/_sent_messages")
        resp.raise_for_status()
        return resp.json()

    def clear(self):
        """清空 Mock Server 的消息记录"""
        httpx.post(f"{self.base_url}/_clear").raise_for_status()

    def wait_for_reply(self, count_before: int = 0,
                       timeout: int = 10) -> dict | None:
        """
        等待 bot 发送新消息，返回最新一条。
        count_before: 调用前已有的消息数量
        timeout: 最长等待秒数
        """
        for _ in range(timeout):
            time.sleep(1)
            msgs = self.get_sent_messages()
            if len(msgs) > count_before:
                return msgs[-1]
        return None

    def wait_for_reply_async(self, count_before: int = 0,
                             timeout: int = 10):
        """异步版本的 wait_for_reply，供 pytest-asyncio 使用"""
        return _AsyncWaiter(self, count_before, timeout)

    def health(self) -> bool:
        """检查 Mock Server 是否在线"""
        try:
            resp = httpx.get(f"{self.base_url}/health", timeout=2)
            return resp.status_code == 200
        except Exception:
            return False


class _AsyncWaiter:
    """供 async with 语法使用的异步等待器"""

    def __init__(self, runner: BotTestRunner, count_before: int, timeout: int):
        self.runner = runner
        self.count_before = count_before
        self.timeout = timeout

    def __await__(self):
        return self._wait().__await__()

    async def _wait(self) -> dict | None:
        for _ in range(self.timeout):
            await asyncio.sleep(1)
            msgs = self.runner.get_sent_messages()
            if len(msgs) > self.count_before:
                return msgs[-1]
        return None
