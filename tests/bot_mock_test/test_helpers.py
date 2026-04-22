#!/usr/bin/env python3
"""
Bot Mock 测试共享工具函数。
"""

import time
from typing import Optional


def inject_and_get_reply(runner, text: str, timeout: int = 15, **inject_kwargs) -> str:
    """注入消息，记录注入前的消息数，等待新回复。

    inject_kwargs 会透传给 runner.inject()，各平台参数不同：
      - Telegram: chat_id=100, username="testuser"
      - Discord:  channel_id="1039178386623557754"
    默认值由各 runner.inject() 方法定义。
    """
    count_before = len(runner.get_sent_messages())
    runner.inject(text, **inject_kwargs)
    
    # Extract identifier for filtering (supports both Telegram and Discord)
    # Telegram uses chat_id, Discord uses channel_id
    filter_id = inject_kwargs.get("chat_id") or inject_kwargs.get("channel_id")
    msg = runner.wait_for_reply(count_before=count_before, timeout=timeout, chat_id=filter_id)
    
    # Truncate long messages in error output to avoid cluttering logs
    text_preview = text[:100] + "..." if len(text) > 100 else text
    assert msg is not None, f"Bot 未在 {timeout}s 内回复 '{text_preview}'"
    return msg["text"]


def wait_for_abort_reply(runner, count_before: int, timeout: int = 15, 
                         chat_id=None, cancel_keywords=None) -> dict:
    """等待 abort/cancel 响应，自动跳过流式状态消息和无关消息。
    
    Args:
        runner: Test runner instance
        count_before: 注入 abort 命令前的消息数量
        timeout: 最长等待时间（秒）
        chat_id: 频道/聊天 ID（用于过滤）
        cancel_keywords: 取消响应的关键词列表，默认为英文和中文
    
    Returns:
        匹配的消息字典
    
    Raises:
        AssertionError: 如果超时未收到取消响应
    """
    if cancel_keywords is None:
        cancel_keywords = [
            "🛑", "cancel", "cancelled", "取消", "已取消",
            "キャンセル", "Отменено"  # 日文和俄文
        ]
    
    # Skip streaming status messages
    streaming_status = [
        "Processing", "Deliberating", "Evaluating", "Connecting", 
        "Thinking", "Considering", "Analyzing", "Working"
    ]
    
    import time
    start_time = time.time()
    poll_interval = 0.5
    
    while time.time() - start_time < timeout:
        # Get all messages sent after count_before
        msgs = runner.get_sent_messages()
        new_msgs = msgs[count_before:]
        
        # Check each new message from oldest to newest
        for msg in new_msgs:
            text = msg["text"]
            
            # Skip streaming status messages
            if text in streaming_status:
                continue
            
            # Check if this is a cancel response
            text_lower = text.lower()
            has_cancel_keyword = any(kw.lower() in text_lower for kw in cancel_keywords if not kw.startswith("🛑"))
            has_emoji = "🛑" in text
            
            if has_cancel_keyword or has_emoji:
                return msg
        
        # No cancel response found yet, wait and retry
        time.sleep(poll_interval)
    
    # Timeout reached
    assert False, f"Timeout waiting for cancel response after {timeout}s. Last messages: {[m['text'][:50] for m in msgs[count_before:][-3:]]}"
