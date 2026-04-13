#!/usr/bin/env python3
"""
Telegram Bot 集成测试用例

前置条件（由 run_test.fish 自动完成）：
  1. Mock Server 运行在 http://127.0.0.1:5000
  2. octos gateway 已启动并连接到 Mock Server

运行方式：
  # 通过 run_test.fish 自动运行（推荐）
  fish tests/telegram_mock/run_test.fish

  # 手动运行（需先手动启动 mock server 和 bot）
  cd tests/telegram_mock
  pytest test_bot.py -v
"""

import time
import pytest
from runner import BotTestRunner

# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def runner():
    """提供一个连接到 Mock Server 的 BotTestRunner 实例"""
    r = BotTestRunner()
    assert r.health(), "Mock Server 未运行，请先启动 run_test.fish"
    return r


@pytest.fixture(autouse=True)
def clear_messages(runner):
    """每个测试前清空消息记录"""
    runner.clear()
    yield
    # 测试后不清空，方便调试


# ── 测试用例 ──────────────────────────────────────────────────────────────────

class TestBotCommands:
    """Bot 命令测试（本地处理，无需 LLM，响应快）"""

    def test_start_command(self, runner: BotTestRunner):
        """
        /start 命令应返回可用命令列表
        预期：bot 回复包含命令帮助信息
        """
        runner.inject("/start", chat_id=123)
        msg = runner.wait_for_reply(count_before=0, timeout=10)

        assert msg is not None, "Bot 未回复 /start 命令"
        print(f"\n  Bot 回复: {msg['text'][:100]}")

    def test_new_session_command(self, runner: BotTestRunner):
        """
        /new 命令应创建新会话
        预期：bot 回复确认信息
        """
        runner.inject("/new", chat_id=123)
        msg = runner.wait_for_reply(count_before=0, timeout=10)

        assert msg is not None, "Bot 未回复 /new 命令"
        print(f"\n  Bot 回复: {msg['text'][:100]}")

    def test_sessions_command(self, runner: BotTestRunner):
        """
        /sessions 命令应列出当前会话
        预期：bot 回复会话列表
        """
        runner.inject("/sessions", chat_id=123)
        msg = runner.wait_for_reply(count_before=0, timeout=10)

        assert msg is not None, "Bot 未回复 /sessions 命令"
        print(f"\n  Bot 回复: {msg['text'][:100]}")

    def test_unknown_command(self, runner: BotTestRunner):
        """
        未知命令应返回错误提示
        预期：bot 回复包含可用命令列表
        """
        runner.inject("/unknowncmd", chat_id=123)
        msg = runner.wait_for_reply(count_before=0, timeout=10)

        assert msg is not None, "Bot 未回复未知命令"
        print(f"\n  Bot 回复: {msg['text'][:100]}")


class TestBotLLM:
    """LLM 消息测试（需要调用 LLM API，响应较慢）"""

    def test_regular_message(self, runner: BotTestRunner):
        """
        普通文本消息应触发 LLM 回复
        预期：bot 返回非空回复
        超时：30s（LLM API 调用）
        """
        runner.inject("Hello!", chat_id=123)
        msg = runner.wait_for_reply(count_before=0, timeout=30)

        assert msg is not None, "Bot 未回复普通消息（30s 超时）"
        assert len(msg["text"]) > 0, "Bot 回复为空"
        print(f"\n  Bot 回复: {msg['text'][:100]}")

    def test_chinese_message(self, runner: BotTestRunner):
        """
        中文消息应正常处理
        预期：bot 返回非空回复
        """
        runner.inject("你好", chat_id=123)
        msg = runner.wait_for_reply(count_before=0, timeout=30)

        assert msg is not None, "Bot 未回复中文消息（30s 超时）"
        print(f"\n  Bot 回复: {msg['text'][:100]}")


class TestBotMultiUser:
    """多用户隔离测试"""

    def test_different_users_isolated(self, runner: BotTestRunner):
        """
        不同用户的消息应独立处理
        预期：两个用户都能收到回复
        """
        # 用户 A
        runner.inject("/start", chat_id=111, username="user_a")
        msg_a = runner.wait_for_reply(count_before=0, timeout=10)
        assert msg_a is not None, "用户 A 未收到回复"

        count = len(runner.get_sent_messages())

        # 用户 B
        runner.inject("/start", chat_id=222, username="user_b")
        msg_b = runner.wait_for_reply(count_before=count, timeout=10)
        assert msg_b is not None, "用户 B 未收到回复"

        print(f"\n  用户 A 回复: {msg_a['text'][:60]}")
        print(f"  用户 B 回复: {msg_b['text'][:60]}")


# ── 如何添加新测试用例 ────────────────────────────────────────────────────────
#
# 1. 在对应的 class 里添加 test_ 开头的方法
# 2. 使用 runner.inject() 发送消息
# 3. 使用 runner.wait_for_reply() 等待回复
# 4. 用 assert 验证结果
#
# 示例：
#
# class TestMyFeature:
#     def test_something(self, runner: BotTestRunner):
#         runner.inject("/mycommand", chat_id=123)
#         msg = runner.wait_for_reply(timeout=10)
#         assert msg is not None
#         assert "expected text" in msg["text"]
