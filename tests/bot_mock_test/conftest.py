#!/usr/bin/env python3
"""
共享 pytest fixtures。

conftest.py 中的 fixture 对同目录下所有测试文件自动生效。
"""

import time
import pytest
import asyncio


def pytest_configure(config):
    """注册自定义 pytest marks，避免警告。"""
    config.addinivalue_line(
        "markers", "llm: marks tests that require LLM API calls (may be slow)"
    )


@pytest.fixture
def event_loop():
    """Create an instance of the default event loop for each test case."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(autouse=True)
def message_baseline(runner):
    """记录测试开始前的消息总数，等待上一个测试可能的延迟回复稳定。

    使用全局单调递增的消息计数，避免 clear() 导致的竞态条件。
    每个 test_*.py 只需定义自己的 runner fixture 即可。
    """
    prev = len(runner.get_sent_messages())
    for _ in range(10):
        time.sleep(0.3)
        cur = len(runner.get_sent_messages())
        if cur == prev:
            break
        prev = cur
    yield


@pytest.fixture(autouse=True)
def llm_test_setup(request, runner):
    """为标记了 @pytest.mark.llm 的测试类自动清理状态。
    
    只在 TestLLMMessages 和 TestDiscordLLMMessages 类中生效。
    """
    # 检查当前测试是否属于 LLM 测试类
    if hasattr(request.node, 'cls') and request.node.cls is not None:
        class_name = request.node.cls.__name__
        if class_name in ('TestLLMMessages', 'TestDiscordLLMMessages'):
            # 在测试开始前清理状态
            runner.clear()
            # 重置到默认会话
            from test_helpers import inject_and_get_reply
            # 根据测试文件确定超时时间（与 test_bot.py 中的 TIMEOUT_COMMAND 保持一致）
            if 'discord' in str(request.node.fspath):
                timeout = 30  # Discord TIMEOUT_COMMAND
            else:
                timeout = 30  # Telegram TIMEOUT_COMMAND
            inject_and_get_reply(runner, "/new", timeout=timeout)
    yield
