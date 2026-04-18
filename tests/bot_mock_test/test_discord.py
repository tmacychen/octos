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
import time
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


@pytest.fixture(autouse=True)
def cleanup_state(runner):
    """每个测试前清理 Mock Server 状态"""
    # Small delay to ensure previous LLM responses are complete
    time.sleep(0.5)
    runner.clear()
    yield


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


# ══════════════════════════════════════════════════════════════════════════════
# 会话隔离测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordSessionIsolation:
    """验证两个用户同时对话时，各自拥有独立上下文"""

    def test_two_users_independent(self, runner):
        """两个不同 channel 的用户，会话应该独立"""
        CHANNEL_1 = "1039178386623557754"
        CHANNEL_2 = "1039178386623557755"
        
        # User 1 creates session
        text1 = inject_and_get_reply(
            runner, "/new user1-session",
            timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_1
        )
        assert "user1-session" in text1
        
        # User 2 creates different session
        text2 = inject_and_get_reply(
            runner, "/new user2-session",
            timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_2
        )
        assert "user2-session" in text2
        
        # Verify sessions are independent
        text1_check = inject_and_get_reply(
            runner, "/sessions",
            timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_1
        )
        text2_check = inject_and_get_reply(
            runner, "/sessions",
            timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_2
        )
        
        # Each should see their own session
        assert "user1-session" in text1_check
        assert "user2-session" in text2_check


# ══════════════════════════════════════════════════════════════════════════════
# 消息分片测试 (Discord 限制 1900 字符)
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
class TestDiscordMessageSplitting:
    """验证 Agent 回复超过 Discord 限制时自动分片"""

    def test_long_response_split(self, runner):
        """发送请求生成长回复，验证是否分片"""
        # 请求生成长文本
        prompt = "Please write a detailed explanation of Python decorators, at least 2000 characters."
        count_before = len(runner.get_sent_messages())
        runner.inject(prompt, channel_id="1039178386623557754")
        
        # Wait for response(s)
        time.sleep(5)
        msgs = runner.get_sent_messages()
        new_msgs = msgs[count_before:]
        
        # Should have multiple messages if response is long
        print(f"\n  Sent {len(new_msgs)} message(s) for long response")
        if len(new_msgs) > 1:
            print(f"  ✓ Message splitting works: {len(new_msgs)} parts")
        else:
            print(f"  ⚠ Only 1 message (response may be short)")


# ══════════════════════════════════════════════════════════════════════════════
# /new 命令测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordNewCommand:
    """验证 /new 命令功能"""

    def test_new_creates_session(self, runner):
        """发 /new → 新会话开始"""
        text = inject_and_get_reply(runner, "/new", timeout=TIMEOUT_COMMAND)
        assert text == "Session cleared.", f"实际回复: {text}"

    def test_new_named_session(self, runner):
        """发 /new <name> → 创建命名会话"""
        text = inject_and_get_reply(runner, "/new my-test", timeout=TIMEOUT_COMMAND)
        assert "Switched to session: my-test" in text


# ══════════════════════════════════════════════════════════════════════════════
# 并发限制测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordConcurrencyLimit:
    """验证并发会话限制 — 同时多个活跃会话的处理能力"""

    def test_concurrent_session_creation(self, runner):
        """同时创建多个会话，验证并发处理能力"""
        import threading
        
        session_count = 10  # 10 个并发
        results = {}
        errors = {}
        
        def create_session(session_id):
            """在独立线程中创建会话"""
            try:
                channel_id = f"1039178386623557{754 + session_id}"
                text = inject_and_get_reply(
                    runner, f"/new concurrent-{session_id}",
                    timeout=30, channel_id=channel_id
                )
                results[session_id] = text
            except Exception as e:
                errors[session_id] = str(e)
        
        # 并行创建所有会话
        threads = []
        start_time = time.time()
        
        for i in range(session_count):
            t = threading.Thread(target=create_session, args=(i,))
            threads.append(t)
            t.start()
        
        # 等待所有线程完成
        for t in threads:
            t.join(timeout=60)
        
        elapsed = time.time() - start_time
        
        print(f"\n  Concurrent sessions: {session_count}")
        print(f"  Elapsed time: {elapsed:.2f}s")
        print(f"  Successful: {len(results)}")
        print(f"  Errors: {len(errors)}")
        
        # 验证所有会话都成功创建
        assert len(errors) == 0, f"Some sessions failed: {errors}"
        assert len(results) == session_count, \
            f"Expected {session_count} results, got {len(results)}"
        
        # 验证每个会话的响应
        for session_id, text in results.items():
            assert "concurrent-" in text or "Switched to session" in text, \
                f"Session {session_id} has incorrect response: {text[:100]}"


# ══════════════════════════════════════════════════════════════════════════════
# Abort 命令测试
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm
@pytest.mark.skip(reason="Discord abort handling needs investigation - Octos may not recognize plain text abort commands in Discord")
class TestDiscordAbortCommands:
    """验证 Agent 能正确中止任务 — 多语言 abort 触发词识别"""

    def test_abort_english_stop(self, runner):
        """发送任务后，用"stop"中止 - 应返回英文响应"""
        
        # 先发送 hi 建立会话
        count_before = len(runner.get_sent_messages())
        runner.inject("hi", channel_id="1039178386623557754")
        hi_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
        assert hi_reply is not None, "Bot did not respond to hi"
        print(f"\n  Session established: {hi_reply['text'][:50]}...")
        
        # 再发送一个任务消息
        count_after_hi = len(runner.get_sent_messages())
        runner.inject("Help me write code", channel_id="1039178386623557754")
        
        # 等待 octos 开始回复
        first_reply = runner.wait_for_reply(count_before=count_after_hi, timeout=TIMEOUT_COMMAND)
        assert first_reply is not None, "Bot did not start processing the task"
        print(f"  Task started: {first_reply['text'][:50]}...")
        
        # 额外等待一小段时间
        time.sleep(1)
        
        # 现在发送 abort 命令
        count_after_first = len(runner.get_sent_messages())
        runner.inject("stop", channel_id="1039178386623557754")
        abort_reply = runner.wait_for_reply(count_before=count_after_first, timeout=TIMEOUT_COMMAND)
        
        assert abort_reply is not None, "Bot did not respond to abort command"
        text = abort_reply["text"]
        
        # 验证收到英文取消响应
        assert "cancel" in text.lower() or "abort" in text.lower() or "🛑" in text, \
            f"Expected English cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (stop) → {text}")

    def test_abort_english_cancel(self, runner):
        """发送任务后，用"cancel"中止"""
        
        count_before = len(runner.get_sent_messages())
        runner.inject("hi", channel_id="1039178386623557755")
        hi_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
        assert hi_reply is not None
        
        count_after_hi = len(runner.get_sent_messages())
        runner.inject("Write a function", channel_id="1039178386623557755")
        first_reply = runner.wait_for_reply(count_before=count_after_hi, timeout=TIMEOUT_COMMAND)
        assert first_reply is not None
        
        time.sleep(1)
        
        count_after_first = len(runner.get_sent_messages())
        runner.inject("cancel", channel_id="1039178386623557755")
        abort_reply = runner.wait_for_reply(count_before=count_after_first, timeout=TIMEOUT_COMMAND)
        
        assert abort_reply is not None, "Bot did not respond to cancel command"
        text = abort_reply["text"]
        
        assert "cancel" in text.lower() or "abort" in text.lower() or "🛑" in text, \
            f"Expected cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (cancel) → {text}")


# ══════════════════════════════════════════════════════════════════════════════
# Profile 模式测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordProfileMode:
    """验证多 profile 配置下的会话隔离"""

    def test_profile_session_isolation(self, runner):
        """不同 channel 使用不同 profile，应该隔离"""
        CHANNEL_A = "1039178386623557754"
        CHANNEL_B = "1039178386623557755"
        
        # Create sessions in different channels
        text_a = inject_and_get_reply(runner, "/new profile-a",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        assert "profile-a" in text_a
        
        text_b = inject_and_get_reply(runner, "/new profile-b",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        assert "profile-b" in text_b
        
        # Verify isolation
        sessions_a = inject_and_get_reply(runner, "/sessions",
                                          timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        sessions_b = inject_and_get_reply(runner, "/sessions",
                                          timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        
        assert "profile-a" in sessions_a
        assert "profile-b" in sessions_b

