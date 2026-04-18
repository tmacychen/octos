#!/usr/bin/env python3
"""
Bot Mock 测试共享工具函数。
"""


def inject_and_get_reply(runner, text: str, timeout: int = 15, **inject_kwargs) -> str:
    """注入消息，记录注入前的消息数，等待新回复。

    inject_kwargs 会透传给 runner.inject()，各平台参数不同：
      - Telegram: chat_id=100, username="testuser"
      - Discord:  channel_id="1039178386623557754"
    默认值由各 runner.inject() 方法定义。
    """
    count_before = len(runner.get_sent_messages())
    runner.inject(text, **inject_kwargs)
    msg = runner.wait_for_reply(count_before=count_before, timeout=timeout)
    
    # Truncate long messages in error output to avoid cluttering logs
    text_preview = text[:100] + "..." if len(text) > 100 else text
    assert msg is not None, f"Bot 未在 {timeout}s 内回复 '{text_preview}'"
    return msg["text"]
