#!/usr/bin/env python3
"""
共享 pytest fixtures。

conftest.py 中的 fixture 对同目录下所有测试文件自动生效。
"""

import time
import pytest


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
