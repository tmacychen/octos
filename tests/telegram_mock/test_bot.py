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
# bot 使用 10s 长轮询，最坏情况下消息要等 10s 才被取到，再加处理时间
TIMEOUT_COMMAND = 15   # 本地命令，无需 LLM
TIMEOUT_LLM     = 30   # 需要调用 LLM API


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def runner():
    r = BotTestRunner()
    assert r.health(), "Mock Server 未运行，请先启动 run_test.fish"
    return r


# 不使用 clear()，改用全局单调递增的消息计数
# 每个测试开始时记录当前消息总数，等待超过该数量的新消息
@pytest.fixture(autouse=True)
def message_baseline(runner):
    """记录测试开始前的消息总数，供 inject_and_get_reply 使用"""
    # 等待上一个测试可能的延迟回复稳定（0.3s 无新消息则认为稳定）
    prev = len(runner.get_sent_messages())
    for _ in range(10):
        time.sleep(0.3)
        cur = len(runner.get_sent_messages())
        if cur == prev:
            break
        prev = cur
    yield


# ── 辅助函数 ──────────────────────────────────────────────────────────────────

def get_reply(runner: BotTestRunner, timeout=TIMEOUT_COMMAND, count_before=0) -> str:
    """等待回复并返回文本，超时则 pytest.fail"""
    msg = runner.wait_for_reply(count_before=count_before, timeout=timeout)
    assert msg is not None, "Bot 未在超时内回复"
    return msg["text"]


def inject_and_get_reply(runner: BotTestRunner, text: str, chat_id: int = 100,
                          username: str = "testuser", timeout=TIMEOUT_COMMAND) -> str:
    """注入消息，记录注入前的消息数，等待新回复"""
    count_before = len(runner.get_sent_messages())
    runner.inject(text, chat_id=chat_id, username=username)
    msg = runner.wait_for_reply(count_before=count_before, timeout=timeout)
    assert msg is not None, f"Bot 未在 {timeout}s 内回复 '{text}'"
    return msg["text"]


# ══════════════════════════════════════════════════════════════════════════════
# 第一层：GatewayDispatcher 命令（会话管理）
# 精确回复来源：crates/octos-cli/src/gateway_dispatcher.rs
# ══════════════════════════════════════════════════════════════════════════════

class TestSessionCommands:
    """会话管理命令 — 本地处理，无需 LLM"""

    def test_new_default(self, runner: BotTestRunner):
        """/new → 'Session cleared.'"""
        text = inject_and_get_reply(runner, "/new")
        assert text == "Session cleared.", f"实际回复: {text}"

    def test_new_named(self, runner: BotTestRunner):
        """/new work → 'Switched to session: work'"""
        text = inject_and_get_reply(runner, "/new work")
        assert text == "Switched to session: work", f"实际回复: {text}"

    def test_new_invalid_name(self, runner: BotTestRunner):
        """/new bad:name（含冒号）→ 'Invalid session name: ...'"""
        text = inject_and_get_reply(runner, "/new bad:name")
        assert text.startswith("Invalid session name:"), f"实际回复: {text}"

    def test_switch_to_existing(self, runner: BotTestRunner):
        """/s <name> → 'Switched to session: <name>'"""
        inject_and_get_reply(runner, "/new research")
        text = inject_and_get_reply(runner, "/s research")
        assert text.startswith("Switched to session: research"), f"实际回复: {text}"

    def test_switch_to_default(self, runner: BotTestRunner):
        """/s（无参数）→ 'Switched to default session.'"""
        text = inject_and_get_reply(runner, "/s")
        assert text == "Switched to default session.", f"实际回复: {text}"

    def test_sessions_list(self, runner: BotTestRunner):
        """/sessions → bot replies with session list (sessions exist from prior tests)"""
        text = inject_and_get_reply(runner, "/sessions")
        assert len(text) > 0, "Empty reply"
        print(f"\n  /sessions → {text[:100]}")

    def test_back_returns_session(self, runner: BotTestRunner):
        """/back → either 'Switched back to session: X' or 'No previous session'"""
        text = inject_and_get_reply(runner, "/back")
        assert "session" in text.lower(), f"Unexpected reply: {text}"
        print(f"\n  /back → {text}")

    def test_back_with_history(self, runner: BotTestRunner):
        """/back（有历史）→ 'Switched back to session: <name>'"""
        inject_and_get_reply(runner, "/new alpha")
        inject_and_get_reply(runner, "/new beta")
        text = inject_and_get_reply(runner, "/back")
        assert text.startswith("Switched back to session:"), f"实际回复: {text}"

    def test_back_alias_b(self, runner: BotTestRunner):
        """/b 与 /back 行为相同"""
        text = inject_and_get_reply(runner, "/b")
        assert "session" in text.lower(), f"实际回复: {text}"

    def test_delete_session(self, runner: BotTestRunner):
        """/delete <name> → 'Deleted session: <name>'"""
        inject_and_get_reply(runner, "/new to-delete")
        text = inject_and_get_reply(runner, "/delete to-delete")
        assert text == "Deleted session: to-delete", f"实际回复: {text}"

    def test_delete_alias_d(self, runner: BotTestRunner):
        """/d <name> 与 /delete 行为相同"""
        inject_and_get_reply(runner, "/new d-alias")
        text = inject_and_get_reply(runner, "/d d-alias")
        assert text == "Deleted session: d-alias", f"实际回复: {text}"

    def test_delete_no_name(self, runner: BotTestRunner):
        """/delete（无参数）不匹配 dispatcher，走未知命令帮助"""
        text = inject_and_get_reply(runner, "/delete")
        assert len(text) > 0, "回复为空"
        print(f"\n  /delete (no arg) → {text[:80]}")

    def test_soul_show_default(self, runner: BotTestRunner):
        """/soul → 显示当前 soul 或默认提示"""
        text = inject_and_get_reply(runner, "/soul")
        assert len(text) > 0, "回复为空"
        print(f"\n  /soul → {text[:80]}")

    def test_soul_set(self, runner: BotTestRunner):
        """/soul <text> → 'Soul updated. Takes effect in new sessions.'"""
        text = inject_and_get_reply(runner, "/soul You are a helpful assistant.")
        assert text == "Soul updated. Takes effect in new sessions.", f"实际回复: {text}"

    def test_soul_reset(self, runner: BotTestRunner):
        """/soul reset → 'Soul reset to default. Takes effect in new sessions.'"""
        text = inject_and_get_reply(runner, "/soul reset")
        assert text == "Soul reset to default. Takes effect in new sessions.", f"实际回复: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 第二层：SessionActor 命令（会话内控制）
# 精确回复来源：crates/octos-cli/src/session_actor.rs
# ══════════════════════════════════════════════════════════════════════════════

class TestSessionActorCommands:
    """会话内控制命令 — 本地处理，无需 LLM"""

    def test_adaptive_no_router(self, runner: BotTestRunner):
        """/adaptive（未启用自适应路由）→ 'Adaptive routing is not enabled.'"""
        text = inject_and_get_reply(runner, "/adaptive")
        assert text == "Adaptive routing is not enabled.", f"实际回复: {text}"

    def test_queue_show(self, runner: BotTestRunner):
        """/queue → 'Queue mode: Collect'（默认模式）"""
        text = inject_and_get_reply(runner, "/queue")
        assert text.startswith("Queue mode:"), f"实际回复: {text}"

    def test_queue_set_followup(self, runner: BotTestRunner):
        """/queue followup → 'Queue mode set to: Followup'"""
        text = inject_and_get_reply(runner, "/queue followup")
        assert "Followup" in text, f"实际回复: {text}"

    def test_queue_set_invalid(self, runner: BotTestRunner):
        """/queue badmode → 'Unknown mode: ...'"""
        text = inject_and_get_reply(runner, "/queue badmode")
        assert "Unknown mode" in text, f"实际回复: {text}"

    def test_status_show(self, runner: BotTestRunner):
        """/status → 显示 Status Config"""
        text = inject_and_get_reply(runner, "/status")
        assert "Status Config" in text, f"实际回复: {text}"

    def test_reset_command(self, runner: BotTestRunner):
        """/reset → 'Reset: queue=collect, adaptive=off, history cleared.'"""
        text = inject_and_get_reply(runner, "/reset")
        assert text == "Reset: queue=collect, adaptive=off, history cleared.", \
            f"实际回复: {text}"

    def test_unknown_command_help(self, runner: BotTestRunner):
        """未知命令 → 帮助文本，包含所有已知命令"""
        text = inject_and_get_reply(runner, "/unknowncmd")
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
        text_a = inject_and_get_reply(runner, "/new user-a-topic", chat_id=201, username="user_a")
        assert text_a == "Switched to session: user-a-topic"

        text_b = inject_and_get_reply(runner, "/new user-b-topic", chat_id=202, username="user_b")
        assert text_b == "Switched to session: user-b-topic"

    def test_callback_session_switch(self, runner: BotTestRunner):
        """内联键盘回调 s:<name> 应切换会话"""
        inject_and_get_reply(runner, "/new cb-topic")

        # 模拟点击按钮（edit_message_with_metadata 不发新消息，只编辑原消息）
        runner.inject_callback("s:cb-topic", chat_id=100, message_id=100)
        time.sleep(2)
        # 不断言新消息，只验证不崩溃
        print(f"\n  callback s:cb-topic processed")


# ══════════════════════════════════════════════════════════════════════════════
# LLM 消息测试（标记 llm，可用 pytest -m "not llm" 跳过）
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
class TestLLMMessages:
    """需要调用 LLM API，超时 TIMEOUT_LLM = 30s"""

    def test_regular_message(self, runner: BotTestRunner):
        text = inject_and_get_reply(runner, "Hello!", timeout=TIMEOUT_LLM)
        assert len(text) > 0

    def test_chinese_message(self, runner: BotTestRunner):
        text = inject_and_get_reply(runner, "你好", timeout=TIMEOUT_LLM)
        assert len(text) > 0
