#!/usr/bin/env python3
"""
共享 pytest fixtures。

conftest.py 中的 fixture 对同目录下所有测试文件自动生效。
"""

import time
import pytest
import asyncio
import sys


def pytest_configure(config):
    """注册自定义 pytest marks，避免警告。"""
    config.addinivalue_line(
        "markers", "llm: marks tests that require LLM API calls (may be slow)"
    )
    config.addinivalue_line(
        "markers", "slow: marks tests that are slow (large data transfer, etc.)"
    )
    config.addinivalue_line(
        "markers", "abort_test: marks abort/cancel tests that need extra cleanup delay"
    )
    config.addinivalue_line(
        "markers", "llm_intensive: marks LLM-intensive tests that need extra delay between runs"
    )
    
    # Suppress httpx INFO logs to reduce noise
    import logging
    logging.getLogger("httpx").setLevel(logging.WARNING)
    logging.getLogger("httpcore").setLevel(logging.WARNING)


def pytest_runtest_logstart(nodeid, location):
    """在每个测试用例开始时打印视觉分隔符到日志文件。"""
    # 提取测试名称（去掉路径和参数）
    test_name = nodeid.split("::")[-1]
    
    # 构建分隔符
    separator = "\n" + "=" * 70 + "\n"
    banner = f"▶ START TEST: {test_name}\n"
    
    # 同时输出到 stdout 和 stderr（会被 tee 捕获）
    print(separator, file=sys.stdout, flush=True)
    print(banner, file=sys.stdout, flush=True)
    print(separator, file=sys.stderr, flush=True)


@pytest.fixture
def event_loop():
    """Create an instance of the default event loop for each test case."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(autouse=True)
def message_baseline(runner):
    """记录测试开始前的消息总数。

    使用全局单调递增的消息计数，避免 clear() 导致的竞态条件。
    每个 test_*.py 只需定义自己的 runner fixture 即可。
    
    Note: cleanup_state fixture already waits for message stability,
    so we just record the baseline here without additional waiting.
    """
    # Just record the current count - cleanup_state already handled stabilization
    prev = len(runner.get_sent_messages())
    yield prev


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
            # 根据测试文件确定超时时间（与 test_telegram.py 中的 TIMEOUT_COMMAND 保持一致）
            if 'discord' in str(request.node.fspath):
                timeout = 30  # Discord TIMEOUT_COMMAND
            else:
                timeout = 30  # Telegram TIMEOUT_COMMAND
            inject_and_get_reply(runner, "/new", timeout=timeout)
    yield
