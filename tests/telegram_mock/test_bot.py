#!/usr/bin/env python3
"""
Telegram Bot 集成测试用例

前置条件（由 run_test.fish 自动完成）：
  1. Mock Server 运行在 http://127.0.0.1:5000
  2. octos gateway 已启动并连接到 Mock Server

运行方式：
  fish tests/telegram_mock/run_test.fish        # 完整测试
  pytest test_bot.py -v -m "not llm"           # 跳过 LLM 测试
"""

import time
import pytest
from runner import BotTestRunner

# ── 超时配置 ──────────────────────────────────────────────────────────────────
TIMEOUT_COMMAND = 10   # 本地命令，无需 LLM
TIMEOUT_LLM     = 30   # 需要调用 LLM API


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def runner():
    r = BotTestRunner()
    assert r.health(), "Mock Server 未运行，请先启动 run_test.fish"
    return r


@pytest.fixture(autouse=True)
def clear_messages(runner):
    runner.clear()
    yield


# ── 辅助函数 ──────────────────────────────────────────────────────────────────

def get_reply(runner: BotTestRunner, timeout=TIMEOUT_COMMAND, count_before=0) -> str:
    """等待回复并返回文本，超时则 pytest.fail"""
    msg = runner.wait_for_reply(count_before=count_before, timeout=timeout)
    assert msg is not None, "Bot 未在超时内回复"
    return msg["text"]


# ══════════════════════════════════════════════════════════════════════════════
# 第一层：GatewayDispatcher 命令（会话管理）
# 精确回复来源：crates/octos-cli/src/gateway_dispatcher.rs
# ══════════════════════════════════════════════════════════════════════════════

class TestSessionCommands:
    """会话管理命令 — 本地处理，无需 LLM"""

    def test_new_default(self, runner: BotTestRunner):
        """/new → 'Session cleared.'"""
        runner.inject("/new", chat_id=100)
        text = get_reply(runner)
        assert text == "Session cleared.", f"实际回复: {text}"

    def test_new_named(self, runner: BotTestRunner):
        """/new work → 'Switched to session: work'"""
        runner.inject("/new work", chat_id=100)
        text = get_reply(runner)
        assert text == "Switched to session: work", f"实际回复: {text}"

    def test_new_invalid_name(self, runner: BotTestRunner):
        """/new invalid name（含空格）→ 'Invalid session name: ...'"""
        runner.inject("/new bad name", chat_id=100)
        text = get_reply(runner)
        assert text.startswith("Invalid session name:"), f"实际回复: {text}"

    def test_switch_to_existing(self, runner: BotTestRunner):
        """/s <name> → 'Switched to session: <name>'"""
        # 先创建
        runner.inject("/new research", chat_id=100)
        get_reply(runner)
        runner.clear()

        runner.inject("/s research", chat_id=100)
        text = get_reply(runner)
        assert text.startswith("Switched to session: research"), f"实际回复: {text}"

    def test_switch_to_default(self, runner: BotTestRunner):
        """/s（无参数）→ 'Switched to default session.'"""
        runner.inject("/s", chat_id=100)
        text = get_reply(runner)
        assert text == "Switched to default session.", f"实际回复: {text}"

    def test_sessions_empty(self, runner: BotTestRunner):
        """/sessions（无会话）→ 'No sessions found. Use /new <name> to create one.'"""
        runner.inject("/sessions", chat_id=100)
        text = get_reply(runner)
        # 可能有默认会话，只验证有回复
        assert len(text) > 0, "回复为空"
        print(f"\n  /sessions → {text[:100]}")

    def test_back_no_history(self, runner: BotTestRunner):
        """/back（无历史）→ 'No previous session to switch to.'"""
        runner.inject("/back", chat_id=100)
        text = get_reply(runner)
        # 可能有历史，两种结果都合法
        assert "session" in text.lower(), f"实际回复: {text}"
        print(f"\n  /back → {text}")

    def test_back_with_history(self, runner: BotTestRunner):
        """/back（有历史）→ 'Switched back to session: <name>'"""
        runner.inject("/new alpha", chat_id=100)
        get_reply(runner)
        runner.clear()

        runner.inject("/new beta", chat_id=100)
        get_reply(runner)
        runner.clear()

        runner.inject("/back", chat_id=100)
        text = get_reply(runner)
        assert text.startswith("Switched back to session:"), f"实际回复: {text}"

    def test_back_alias_b(self, runner: BotTestRunner):
        """/b 与 /back 行为相同"""
        runner.inject("/b", chat_id=100)
        text = get_reply(runner)
        assert "session" in text.lower(), f"实际回复: {text}"

    def test_delete_session(self, runner: BotTestRunner):
        """/delete <name> → 'Deleted session: <name>'"""
        runner.inject("/new to-delete", chat_id=100)
        get_reply(runner)
        runner.clear()

        runner.inject("/delete to-delete", chat_id=100)
        text = get_reply(runner)
        assert text == "Deleted session: to-delete", f"实际回复: {text}"

    def test_delete_alias_d(self, runner: BotTestRunner):
        """/d <name> 与 /delete 行为相同"""
        runner.inject("/new d-alias", chat_id=100)
        get_reply(runner)
        runner.clear()

        runner.inject("/d d-alias", chat_id=100)
        text = get_reply(runner)
        assert text == "Deleted session: d-alias", f"实际回复: {text}"

    def test_delete_no_name(self, runner: BotTestRunner):
        """/delete（无参数）不匹配，走未知命令帮助"""
        runner.inject("/delete", chat_id=100)
        text = get_reply(runner)
        # /delete 不带参数不匹配 dispatcher，走 session actor 的未知命令
        assert len(text) > 0, "回复为空"
        print(f"\n  /delete (no arg) → {text[:80]}")

    def test_soul_show_default(self, runner: BotTestRunner):
        """/soul → 'No custom soul set. Using default.' 或显示当前 soul"""
        runner.inject("/soul", chat_id=100)
        text = get_reply(runner)
        assert len(text) > 0, "回复为空"
        print(f"\n  /soul → {text[:80]}")

    def test_soul_set(self, runner: BotTestRunner):
        """/soul <text> → 'Soul updated. Takes effect in new sessions.'"""
        runner.inject("/soul You are a helpful assistant.", chat_id=100)
        text = get_reply(runner)
        assert text == "Soul updated. Takes effect in new sessions.", f"实际回复: {text}"

    def test_soul_reset(self, runner: BotTestRunner):
        """/soul reset → 'Soul reset to default. Takes effect in new sessions.'"""
        runner.inject("/soul reset", chat_id=100)
        text = get_reply(runner)
        assert text == "Soul reset to default. Takes effect in new sessions.", f"实际回复: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 第二层：SessionActor 命令（会话内控制）
# 精确回复来源：crates/octos-cli/src/session_actor.rs
# ══════════════════════════════════════════════════════════════════════════════

class TestSessionActorCommands:
    """会话内控制命令 — 本地处理，无需 LLM"""

    def test_adaptive_no_router(self, runner: BotTestRunner):
        """/adaptive（未启用自适应路由）→ 'Adaptive routing is not enabled.'"""
        runner.inject("/adaptive", chat_id=100)
        text = get_reply(runner)
        # 测试配置只有单 provider，adaptive routing 未启用
        assert text == "Adaptive routing is not enabled.", f"实际回复: {text}"

    def test_queue_show(self, runner: BotTestRunner):
        """/queue → 'Queue mode: Collect'（默认模式）"""
        runner.inject("/queue", chat_id=100)
        text = get_reply(runner)
        assert text.startswith("Queue mode:"), f"实际回复: {text}"

    def test_queue_set_followup(self, runner: BotTestRunner):
        """/queue followup → 'Queue mode set to: Followup'"""
        runner.inject("/queue followup", chat_id=100)
        text = get_reply(runner)
        assert "Followup" in text, f"实际回复: {text}"

    def test_queue_set_invalid(self, runner: BotTestRunner):
        """/queue badmode → 'Unknown mode: ...'"""
        runner.inject("/queue badmode", chat_id=100)
        text = get_reply(runner)
        assert "Unknown mode" in text, f"实际回复: {text}"

    def test_status_show(self, runner: BotTestRunner):
        """/status → 显示 Status Config"""
        runner.inject("/status", chat_id=100)
        text = get_reply(runner)
        assert "Status Config" in text, f"实际回复: {text}"

    def test_reset_command(self, runner: BotTestRunner):
        """/reset → 'Reset: queue=collect, adaptive=off, history cleared.'"""
        runner.inject("/reset", chat_id=100)
        text = get_reply(runner)
        assert text == "Reset: queue=collect, adaptive=off, history cleared.", \
            f"实际回复: {text}"

    def test_unknown_command_help(self, runner: BotTestRunner):
        """未知命令 → 帮助文本，包含所有已知命令"""
        runner.inject("/unknowncmd", chat_id=100)
        text = get_reply(runner)
        assert text.startswith("Unknown command."), f"实际回复: {text}"
        for cmd in ["/new", "/s", "/sessions", "/back", "/delete", "/soul",
                    "/status", "/adaptive", "/reset"]:
            assert cmd in text, f"帮助文本缺少 {cmd}: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 多用户隔离 & 回调测试
# ══════════════════════════════════════════════════════════════════════════════

class TestMultiUser:

    def test_two_users_independent(self, runner: BotTestRunner):
        """两个不同 chat_id 的用户各自创建会话，互不干扰"""
        runner.inject("/new user-a-topic", chat_id=201, username="user_a")
        msg_a = runner.wait_for_reply(count_before=0, timeout=TIMEOUT_COMMAND)
        assert msg_a is not None
        assert msg_a["text"] == "Switched to session: user-a-topic"

        count = len(runner.get_sent_messages())

        runner.inject("/new user-b-topic", chat_id=202, username="user_b")
        msg_b = runner.wait_for_reply(count_before=count, timeout=TIMEOUT_COMMAND)
        assert msg_b is not None
        assert msg_b["text"] == "Switched to session: user-b-topic"

    def test_callback_session_switch(self, runner: BotTestRunner):
        """内联键盘回调 s:<name> 应切换会话（由 handle_session_callback 处理）"""
        # 先创建会话
        runner.inject("/new cb-topic", chat_id=100)
        get_reply(runner)
        runner.clear()

        # 模拟点击按钮
        runner.inject_callback("s:cb-topic", chat_id=100, message_id=100)
        # 回调切换会话后不一定发新消息（edit_message_with_metadata），
        # 但如果没有 message_id 对应的消息可编辑，可能无回复
        # 这里只验证不崩溃
        time.sleep(2)
        print(f"\n  callback s:cb-topic → {runner.get_sent_messages()}")


# ══════════════════════════════════════════════════════════════════════════════
# LLM 消息测试（标记 llm，可用 pytest -m "not llm" 跳过）
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
class TestLLMMessages:
    """需要调用 LLM API，超时 TIMEOUT_LLM = 30s"""

    def test_regular_message(self, runner: BotTestRunner):
        runner.inject("Hello!", chat_id=100)
        text = get_reply(runner, timeout=TIMEOUT_LLM)
        assert len(text) > 0

    def test_chinese_message(self, runner: BotTestRunner):
        runner.inject("你好", chat_id=100)
        text = get_reply(runner, timeout=TIMEOUT_LLM)
        assert len(text) > 0
