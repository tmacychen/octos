#!/usr/bin/env python3
"""
BotTestRunner - 连接已运行的 Telegram Mock Server，供 pytest 测试用例使用。
"""

import os

import httpx

from base_runner import BaseMockRunner

MOCK_BASE_URL = os.environ.get("MOCK_BASE_URL", "http://127.0.0.1:5000")


class BotTestRunner(BaseMockRunner):
    """Telegram Mock Server 测试辅助工具。"""

    def __init__(self, base_url: str = MOCK_BASE_URL):
        super().__init__(base_url)

    def inject(self, text: str, chat_id: int = 123,
               username: str = "testuser") -> dict:
        """向 Mock Server 注入一条用户消息"""
        resp = httpx.post(f"{self.base_url}/_inject", json={
            "text": text,
            "chat_id": chat_id,
            "username": username,
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

    def inject_document(self, file_path: str, caption: str = "",
                       chat_id: int = 123, username: str = "testuser") -> dict:
        """注入一个文档上传（模拟文件发送）
        
        Args:
            file_path: 本地文件路径
            caption: 文件说明文字
            chat_id: 聊天ID
            username: 用户名
        
        Returns:
            Mock Server响应
        """
        resp = httpx.post(f"{self.base_url}/_inject_document", json={
            "file_path": file_path,
            "caption": caption,
            "chat_id": chat_id,
            "username": username,
        }, timeout=10)
        resp.raise_for_status()
        return resp.json()

    def get_edit_history(self) -> list[dict]:
        """获取消息编辑历史（测试流式编辑用）"""
        resp = httpx.get(f"{self.base_url}/_edit_history", timeout=10)
        resp.raise_for_status()
        return resp.json()
    
    def clear_edit_history(self):
        """清空编辑历史"""
        # clear() 已经清空了编辑历史
        pass
