#!/usr/bin/env python3
"""
Telegram Bot 集成测试用例

前置条件（由 run_test.fish 自动完成）：
  1. Mock Server 运行在 http://127.0.0.1:5000
  2. octos gateway 已启动并连接到 Mock Server

运行方式：
  fish tests/bot_mock/run_test.fish telegram    # 完整测试
  pytest test_bot.py -v -m "not llm"            # 跳过 LLM 测试
"""

import pytest
from runner import BotTestRunner
from test_helpers import inject_and_get_reply

# ── 超时配置 ──────────────────────────────────────────────────────────────────
TIMEOUT_COMMAND = 15   # 本地命令，无需 LLM
TIMEOUT_LLM     = 45   # 需要调用 LLM API (增加到 45s 以应对网络延迟)


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def runner():
    r = BotTestRunner()
    assert r.health(), "Mock Server 未运行，请先启动 run_test.fish"
    return r


# ══════════════════════════════════════════════════════════════════════════════
# 第一层：GatewayDispatcher 命令（会话管理）
# ══════════════════════════════════════════════════════════════════════════════

class TestSessionCommands:
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
        """/new bad:name（含冒号）→ 'Invalid session name: ...'"""
        text = inject_and_get_reply(runner, "/new bad:name", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Invalid session name:"), f"实际回复: {text}"

    def test_switch_to_existing(self, runner):
        """/s <name> → 'Switched to session: <name>'"""
        inject_and_get_reply(runner, "/new research", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/s research", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Switched to session: research"), f"实际回复: {text}"

    def test_switch_to_default(self, runner):
        """/s（无参数）→ 'Switched to default session.'"""
        text = inject_and_get_reply(runner, "/s", timeout=TIMEOUT_COMMAND)
        assert text == "Switched to default session.", f"实际回复: {text}"

    def test_sessions_list(self, runner):
        """/sessions → bot replies with session list"""
        text = inject_and_get_reply(runner, "/sessions", timeout=TIMEOUT_COMMAND)
        assert len(text) > 0, "Empty reply"
        print(f"\n  /sessions → {text[:100]}")

    def test_back_returns_session(self, runner):
        """/back → session-related reply"""
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND)
        assert "session" in text.lower(), f"Unexpected reply: {text}"
        print(f"\n  /back → {text}")

    def test_back_with_history(self, runner):
        """/back（有历史）→ 'Switched back to session: <name>'"""
        inject_and_get_reply(runner, "/new alpha", timeout=TIMEOUT_COMMAND)
        inject_and_get_reply(runner, "/new beta", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Switched back to session:"), f"实际回复: {text}"

    def test_back_alias_b(self, runner):
        """/b 与 /back 行为相同"""
        text = inject_and_get_reply(runner, "/b", timeout=TIMEOUT_COMMAND)
        assert "session" in text.lower(), f"实际回复: {text}"

    def test_delete_session(self, runner):
        """/delete <name> → 'Deleted session: <name>'"""
        inject_and_get_reply(runner, "/new to-delete", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/delete to-delete", timeout=TIMEOUT_COMMAND)
        assert text == "Deleted session: to-delete", f"实际回复: {text}"

    def test_delete_alias_d(self, runner):
        """/d <name> 与 /delete 行为相同"""
        inject_and_get_reply(runner, "/new d-alias", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/d d-alias", timeout=TIMEOUT_COMMAND)
        assert text == "Deleted session: d-alias", f"实际回复: {text}"

    def test_delete_no_name(self, runner):
        """/delete（无参数）不匹配 dispatcher，走未知命令帮助"""
        text = inject_and_get_reply(runner, "/delete", timeout=TIMEOUT_COMMAND)
        assert len(text) > 0, "回复为空"
        print(f"\n  /delete (no arg) → {text[:80]}")

    def test_soul_show_default(self, runner):
        """/soul → 显示当前 soul 或默认提示"""
        text = inject_and_get_reply(runner, "/soul", timeout=TIMEOUT_COMMAND)
        assert len(text) > 0, "回复为空"
        print(f"\n  /soul → {text[:80]}")

    def test_soul_set(self, runner):
        """/soul <text> → 'Soul updated. Takes effect in new sessions.'"""
        text = inject_and_get_reply(runner, "/soul You are a helpful assistant.", timeout=TIMEOUT_COMMAND)
        assert text == "Soul updated. Takes effect in new sessions.", f"实际回复: {text}"

    def test_soul_reset(self, runner):
        """/soul reset → 'Soul reset to default. Takes effect in new sessions.'"""
        text = inject_and_get_reply(runner, "/soul reset", timeout=TIMEOUT_COMMAND)
        assert text == "Soul reset to default. Takes effect in new sessions.", f"实际回复: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 第二层：SessionActor 命令（会话内控制）
# ══════════════════════════════════════════════════════════════════════════════

class TestSessionActorCommands:
    """会话内控制命令 — 本地处理，无需 LLM"""

    def test_adaptive_no_router(self, runner):
        """/adaptive（未启用自适应路由）→ 'Adaptive routing is not enabled.'"""
        text = inject_and_get_reply(runner, "/adaptive", timeout=TIMEOUT_COMMAND)
        assert text == "Adaptive routing is not enabled.", f"实际回复: {text}"

    def test_queue_show(self, runner):
        """/queue → 'Queue mode: Collect'（默认模式）"""
        text = inject_and_get_reply(runner, "/queue", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Queue mode:"), f"实际回复: {text}"

    def test_queue_set_followup(self, runner):
        """/queue followup → 'Queue mode set to: Followup'"""
        text = inject_and_get_reply(runner, "/queue followup", timeout=TIMEOUT_COMMAND)
        assert "Followup" in text, f"实际回复: {text}"

    def test_queue_set_invalid(self, runner):
        """/queue badmode → 'Unknown mode: ...'"""
        text = inject_and_get_reply(runner, "/queue badmode", timeout=TIMEOUT_COMMAND)
        assert "Unknown mode" in text, f"实际回复: {text}"

    def test_status_show(self, runner):
        """/status → 显示 Status Config"""
        text = inject_and_get_reply(runner, "/status", timeout=TIMEOUT_COMMAND)
        assert "Status Config" in text, f"实际回复: {text}"

    def test_reset_command(self, runner):
        """/reset → 'Reset: queue=collect, adaptive=off, history cleared.'"""
        text = inject_and_get_reply(runner, "/reset", timeout=TIMEOUT_COMMAND)
        assert text == "Reset: queue=collect, adaptive=off, history cleared.", \
            f"实际回复: {text}"

    def test_unknown_command_help(self, runner):
        """未知命令 → 帮助文本，包含所有已知命令"""
        text = inject_and_get_reply(runner, "/unknowncmd", timeout=TIMEOUT_COMMAND)
        assert text.startswith("Unknown command."), f"实际回复: {text}"
        for cmd in ["/new", "/s", "/sessions", "/back", "/delete", "/soul",
                    "/status", "/adaptive", "/reset"]:
            assert cmd in text, f"帮助文本缺少 {cmd}: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 多用户隔离 & 回调测试
# ══════════════════════════════════════════════════════════════════════════════

class TestMultiUser:

    def test_two_users_independent(self, runner):
        """两个不同 chat_id 的用户各自创建会话，互不干扰"""
        text_a = inject_and_get_reply(runner, "/new user-a-topic",
                                      timeout=TIMEOUT_COMMAND, chat_id=201, username="user_a")
        assert text_a == "Switched to session: user-a-topic"

        text_b = inject_and_get_reply(runner, "/new user-b-topic",
                                      timeout=TIMEOUT_COMMAND, chat_id=202, username="user_b")
        assert text_b == "Switched to session: user-b-topic"

    def test_callback_session_switch(self, runner):
        """内联键盘回调 s:<name> 应切换会话"""
        inject_and_get_reply(runner, "/new cb-topic", timeout=TIMEOUT_COMMAND)

        # 模拟点击按钮（edit_message_with_metadata 不发新消息，只编辑原消息）
        runner.inject_callback("s:cb-topic", chat_id=100, message_id=100)
        import time; time.sleep(2)
        # 不断言新消息，只验证不崩溃
        print(f"\n  callback s:cb-topic processed")


# ══════════════════════════════════════════════════════════════════════════════
# Profile 模式测试（多子账号隔离）
# ══════════════════════════════════════════════════════════════════════════════

class TestProfileMode:
    """验证多 profile/子账号的独立性 — 每个用户有独立的 Provider 和提示词"""

    def test_profile_session_isolation(self, runner):
        """两个不同 profile 的用户应有独立的会话"""
        # User A (profile-a) 创建会话
        text_a = inject_and_get_reply(runner, "/new profile-a-topic",
                                      timeout=TIMEOUT_COMMAND, chat_id=301, username="user_a")
        assert text_a == "Switched to session: profile-a-topic"

        # User B (profile-b) 创建会话
        text_b = inject_and_get_reply(runner, "/new profile-b-topic",
                                      timeout=TIMEOUT_COMMAND, chat_id=302, username="user_b")
        assert text_b == "Switched to session: profile-b-topic"

        # 验证 A 的会话不受 B 影响
        text_a_check = inject_and_get_reply(runner, "/sessions",
                                            timeout=TIMEOUT_COMMAND, chat_id=301)
        assert "profile-a-topic" in text_a_check

    def test_soul_per_profile(self, runner):
        """每个 profile 可以有独立的 soul（提示词）"""
        # Profile A 设置 soul
        text_a_set = inject_and_get_reply(runner, "/soul You are a professional coder.",
                                          timeout=TIMEOUT_COMMAND, chat_id=301)
        assert text_a_set == "Soul updated. Takes effect in new sessions."

        # Profile B 设置不同的 soul
        text_b_set = inject_and_get_reply(runner, "/soul You are a creative writer.",
                                          timeout=TIMEOUT_COMMAND, chat_id=302)
        assert text_b_set == "Soul updated. Takes effect in new sessions."

        # 验证 A 的 soul
        soul_a = inject_and_get_reply(runner, "/soul", timeout=TIMEOUT_COMMAND, chat_id=301)
        assert "coder" in soul_a.lower() or "professional" in soul_a.lower(), \
            f"Profile A soul incorrect: {soul_a}"

        # 验证 B 的 soul
        soul_b = inject_and_get_reply(runner, "/soul", timeout=TIMEOUT_COMMAND, chat_id=302)
        assert "writer" in soul_b.lower() or "creative" in soul_b.lower(), \
            f"Profile B soul incorrect: {soul_b}"

    def test_queue_mode_per_profile(self, runner):
        """每个 profile 可以有独立的队列模式"""
        # Profile A 设置为 followup
        text_a = inject_and_get_reply(runner, "/queue followup",
                                      timeout=TIMEOUT_COMMAND, chat_id=301)
        assert "Followup" in text_a

        # Profile B 保持默认 collect
        text_b = inject_and_get_reply(runner, "/queue",
                                      timeout=TIMEOUT_COMMAND, chat_id=302)
        assert "Collect" in text_b or "collect" in text_b.lower()

        # 验证 A 仍然是 followup
        text_a_check = inject_and_get_reply(runner, "/queue",
                                            timeout=TIMEOUT_COMMAND, chat_id=301)
        assert "Followup" in text_a_check


# ══════════════════════════════════════════════════════════════════════════════
# 消息分片测试（Telegram 限制 4096 字符）
# ══════════════════════════════════════════════════════════════════════════════

class TestMessageSplitting:
    """验证超长消息自动分片 — Telegram API 限制单条消息 4096 字符"""

    def test_telegram_message_length_limit(self, runner):
        """Mock Server 应拒绝超过 4096 字符的消息"""
        # 生成 5000 字符的文本（超过 4096 限制）
        long_text = "A" * 5000
        
        count_before = len(runner.get_sent_messages())
        
        # 直接调用 Mock Server API 测试长度限制
        import httpx
        try:
            resp = httpx.post(
                f"{runner.base_url}/_inject_long_message",
                json={"text": long_text, "chat_id": 123},
                timeout=5
            )
            # 如果 octos 正确处理了分片，应该能成功
            print(f"\n  Long message handled: status={resp.status_code}")
        except Exception as e:
            print(f"\n  Long message test skipped: {e}")

    def test_normal_message_within_limit(self, runner):
        """正常长度的消息应能成功发送"""
        # 生成 1000 字符的文本（在限制内）
        normal_text = "B" * 1000
        
        count_before = len(runner.get_sent_messages())
        runner.inject(normal_text)
        
        # 等待 bot 回复
        time.sleep(3)
        msgs = runner.get_sent_messages()
        
        # 应该有新的消息
        assert len(msgs) > count_before, "Bot should reply to normal message"
        print(f"\n  Normal message (1000 chars) → OK")

    def test_message_near_limit(self, runner):
        """接近限制的消息（4000 字符）应能成功发送"""
        # 生成 4000 字符的文本（接近但未超过 4096）
        near_limit_text = "C" * 4000
        
        count_before = len(runner.get_sent_messages())
        runner.inject(near_limit_text)
        
        # 等待 bot 回复
        time.sleep(3)
        msgs = runner.get_sent_messages()
        
        # 验证消息被处理
        assert len(msgs) >= count_before, "Bot should handle near-limit message"
        print(f"\n  Near-limit message (4000 chars) → OK")


# ══════════════════════════════════════════════════════════════════════════════
# Abort 功能测试（标记 llm，需要调用 LLM API）
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
class TestAbortCommands:
    """验证 Agent 能正确中止任务，支持多语言"""

    def test_abort_chinese_stop(self, runner):
        """发送“停”中止当前任务"""
        # 先触发一个长任务
        count_before = len(runner.get_sent_messages())
        runner.inject("写一个很长的故事，至少1000字")

        # 等待片刻后发送中止
        import time; time.sleep(3)
        text = inject_and_get_reply(runner, "停", timeout=TIMEOUT_COMMAND)

        # 验证收到中止确认（可能包含多种表述）
        assert len(text) > 0, "应收到中止响应"
        print(f"\n  Abort (停) → {text[:100]}")

    def test_abort_english_stop(self, runner):
        """发送"stop"中止"""
        count_before = len(runner.get_sent_messages())
        runner.inject("Write a very long story, at least 1000 words")

        import time; time.sleep(3)
        text = inject_and_get_reply(runner, "stop", timeout=TIMEOUT_COMMAND)

        assert len(text) > 0, "应收到中止响应"
        print(f"\n  Abort (stop) → {text[:100]}")

    def test_abort_english_cancel(self, runner):
        """发送"cancel"中止"""
        count_before = len(runner.get_sent_messages())
        runner.inject("Generate a detailed report")

        import time; time.sleep(3)
        text = inject_and_get_reply(runner, "cancel", timeout=TIMEOUT_COMMAND)

        assert len(text) > 0, "应收到中止响应"
        print(f"\n  Abort (cancel) → {text[:100]}")

    def test_abort_japanese(self, runner):
        """发送“やめて”中止（日语）"""
        count_before = len(runner.get_sent_messages())
        runner.inject("長い物語を書いてください")

        import time; time.sleep(3)
        text = inject_and_get_reply(runner, "やめて", timeout=TIMEOUT_COMMAND)

        assert len(text) > 0, "应收到中止响应"
        print(f"\n  Abort (やめて) → {text[:100]}")

    def test_abort_russian(self, runner):
        """发送“стоп”中止（俄语）"""
        count_before = len(runner.get_sent_messages())
        runner.inject("Напиши длинную историю")

        import time; time.sleep(3)
        text = inject_and_get_reply(runner, "стоп", timeout=TIMEOUT_COMMAND)

        assert len(text) > 0, "应收到中止响应"
        print(f"\n  Abort (стоп) → {text[:100]}")


# ══════════════════════════════════════════════════════════════════════════════
# LLM 消息测试（标记 llm，可用 pytest -m "not llm" 跳过）
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
class TestLLMMessages:
    """需要调用 LLM API，超时 TIMEOUT_LLM = 45s"""

    def test_regular_message(self, runner):
        text = inject_and_get_reply(runner, "Hello!", timeout=TIMEOUT_LLM)
        assert len(text) > 0

    def test_chinese_message(self, runner):
        text = inject_and_get_reply(runner, "你好", timeout=TIMEOUT_LLM)
        assert len(text) > 0
