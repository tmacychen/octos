#!/usr/bin/env python3
"""
Discord Bot 集成测试用例

前置条件（由 run_test.fish 自动完成）：
  1. Mock Discord Server 运行在 http://127.0.0.1:5001 (REST + WS Gateway)
  2. octos gateway 已启动并连接到 Mock Server（通过 --features discord）

运行方式：
  fish tests/bot_mock/run_test.fish discord    # 完整测试
  pytest test_discord.py -v -m "not llm"       # 跳过 LLM 测试
"""

import pytest
from runner_discord import DiscordTestRunner
from test_helpers import inject_and_get_reply

# ── 超时配置 ──────────────────────────────────────────────────────────────────
TIMEOUT_COMMAND = 20   # 本地命令，无需 LLM
TIMEOUT_LLM     = 50   # 需要调用 LLM API (增加到 50s，Discord Gateway 有额外开销)


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def runner():
    r = DiscordTestRunner()
    assert r.health(), "Discord Mock Server 未运行，请先启动 run_test.fish discord"
    return r


# ══════════════════════════════════════════════════════════════════════════════
# 第一层：会话管理命令 (GatewayDispatcher)
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordSessionCommands:
    """会话管理命令 — 本地处理，无需 LLM"""

    def test_new_default(self, runner):
        """/new → 'Session cleared.'"""
        text = inject_and_get_reply(runner, "/new", timeout=TIMEOUT_COMMAND)
        assert text == "Session cleared.", f"实际回复: {text}"

    def test_new_named(self, runner):
        """/new work → 'Switched to session: work'"""
        text = inject_and_get_reply(runner, "/new work", timeout=TIMEOUT_COMMAND)
        assert text == "Switched to session: work", f"实际回复: {text}"

    def test_new_invalid_name(self, runner):
        """/new bad:name → 'Invalid session name:'"""
        text = inject_and_get_reply(runner, "/new bad:name", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Invalid session name:"), f"实际回复: {text}"

    def test_switch_to_existing(self, runner):
        """/s <name> → 'Switched to session: <name>'"""
        inject_and_get_reply(runner, "/new research", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/s research", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Switched to session:"), f"实际回复: {text}"

    def test_switch_to_default(self, runner):
        """/s → 'Switched to default session.'"""
        text = inject_and_get_reply(runner, "/s", timeout=TIMEOUT_COMMAND)
        assert text == "Switched to default session.", f"实际回复: {text}"

    def test_sessions_list(self, runner):
        """/sessions → non-empty reply"""
        text = inject_and_get_reply(runner, "/sessions", timeout=TIMEOUT_COMMAND)
        assert len(text) > 0, "Empty reply"

    def test_back_returns_session(self, runner):
        """/back → session-related reply"""
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND)
        assert "session" in text.lower(), f"Unexpected reply: {text}"

    def test_delete_session(self, runner):
        """/delete <name> → success"""
        inject_and_get_reply(runner, "/new to-delete", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/delete to-delete", timeout=TIMEOUT_COMMAND)
        assert text == "Deleted session: to-delete", f"实际回复: {text}"

    def test_soul_show(self, runner):
        """/soul → non-empty reply"""
        text = inject_and_get_reply(runner, "/soul", timeout=TIMEOUT_COMMAND)
        assert len(text) > 0, "Empty reply"

    def test_soul_set(self, runner):
        """/soul <text> → confirmation"""
        text = inject_and_get_reply(runner, "/soul You are helpful.", timeout=TIMEOUT_COMMAND)
        assert text == "Soul updated. Takes effect in new sessions.", f"实际回复: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 第二层：会话内控制命令 (SessionActor)
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordSessionActorCommands:
    """会话内控制命令 — 本地处理，无需 LLM"""

    def test_adaptive_show(self, runner):
        """/adaptive → not enabled"""
        text = inject_and_get_reply(runner, "/adaptive", timeout=TIMEOUT_COMMAND)
        assert text == "Adaptive routing is not enabled.", f"实际回复: {text}"

    def test_queue_show(self, runner):
        """/queue → Queue mode info"""
        text = inject_and_get_reply(runner, "/queue", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Queue mode:"), f"实际回复: {text}"

    def test_status_show(self, runner):
        """/status → Status Config"""
        text = inject_and_get_reply(runner, "/status", timeout=TIMEOUT_COMMAND)
        assert "Status Config" in text, f"实际回复: {text}"

    def test_reset_command(self, runner):
        """/reset → reset confirmation"""
        text = inject_and_get_reply(runner, "/reset", timeout=TIMEOUT_COMMAND)
        assert text == "Reset: queue=collect, adaptive=off, history cleared.", \
            f"实际回复: {text}"

    def test_unknown_command_help(self, runner):
        """未知命令 → 帮助文本"""
        text = inject_and_get_reply(runner, "/unknowncmd", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Unknown command."), f"实际回复: {text}"
        for cmd in ["/new", "/s", "/sessions"]:
            assert cmd in text, f"帮助文本缺少 {cmd}: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 多用户隔离测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordMultiUser:
    """多用户隔离 & 不同频道隔离"""

    def test_two_channels_independent(self, runner):
        """两个不同 channel_id 的对话互不干扰"""
        CHANNEL_A = "1039178386623557754"
        CHANNEL_B = "200000000000000001"

        text_a = inject_and_get_reply(runner, "/new topic-a",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        assert text_a == "Switched to session: topic-a"

        text_b = inject_and_get_reply(runner, "/new topic-b",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        assert text_b == "Switched to session: topic-b"


# ══════════════════════════════════════════════════════════════════════════════
# LLM 消息测试（标记 @pytest.mark.llm）
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
class TestDiscordLLMMessages:
    """需要调用 LLM API，超时 TIMEOUT_LLM = 50s"""

    def test_regular_message(self, runner):
        """普通英文消息触发 LLM 回复"""
        text = inject_and_get_reply(runner, "Hello!", timeout=TIMEOUT_LLM)
        assert len(text) > 0

    def test_chinese_message(self, runner):
        """中文消息触发 LLM 回复"""
        text = inject_and_get_reply(runner, "你好", timeout=TIMEOUT_LLM)
        assert len(text) > 0
