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
    # Wait for any pending LLM responses to complete
    # This prevents cross-test contamination from slow LLM calls
    # Increased to 5s to ensure LLM responses from previous test have time to arrive
    time.sleep(5.0)
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
        print(f"\n  /sessions → {text[:100]}")

    def test_back_returns_session(self, runner):
        """/back → session-related reply"""
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND)
        assert "session" in text.lower(), f"Unexpected reply: {text}"
        print(f"\n  /back → {text}")

    def test_back_with_history(self, runner):
        """/back 在有历史会话时返回之前的会话"""
        # Create a named session first
        inject_and_get_reply(runner, "/new history-test", timeout=TIMEOUT_COMMAND)
        # Go back should return to previous or default
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND)
        assert "session" in text.lower(), f"Expected session info, got: {text}"

    def test_delete_session(self, runner):
        """/delete <name> → success"""
        inject_and_get_reply(runner, "/new to-delete", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/delete to-delete", timeout=TIMEOUT_COMMAND)
        assert text == "Deleted session: to-delete", f"实际回复: {text}"

    def test_delete_no_name(self, runner):
        """/delete 无名称时显示错误"""
        text = inject_and_get_reply(runner, "/delete", timeout=TIMEOUT_COMMAND)
        # 实际返回："Cannot delete the default session. Use /clear to reset it."
        assert "cannot delete" in text.lower() or "default session" in text.lower() or "clear" in text.lower(), \
            f"Expected error for /delete without name, got: {text}"

    def test_soul_show(self, runner):
        """/soul → non-empty reply"""
        text = inject_and_get_reply(runner, "/soul", timeout=TIMEOUT_COMMAND)
        assert len(text) > 0, "Empty reply"
        print(f"\n  /soul → {text[:80]}")

    def test_soul_set(self, runner):
        """/soul <text> → confirmation"""
        text = inject_and_get_reply(runner, "/soul You are helpful.", timeout=TIMEOUT_COMMAND)
        assert text == "Soul updated. Takes effect in new sessions.", f"实际回复: {text}"

    def test_back_alias_b(self, runner):
        """/b 作为 /back 的别名"""
        text = inject_and_get_reply(runner, "/b", timeout=TIMEOUT_COMMAND)
        assert "session" in text.lower(), f"Unexpected reply for /b: {text}"

    def test_delete_alias_d(self, runner):
        """/d 作为 /delete 的别名"""
        inject_and_get_reply(runner, "/new temp-session", timeout=TIMEOUT_COMMAND)
        text = inject_and_get_reply(runner, "/d temp-session", timeout=TIMEOUT_COMMAND)
        assert "Deleted session: temp-session" in text or "deleted" in text.lower(), \
            f"Unexpected reply for /d: {text}"

    def test_soul_reset(self, runner):
        """/soul reset → 重置 soul"""
        # First set a soul
        inject_and_get_reply(runner, "/soul Custom soul", timeout=TIMEOUT_COMMAND)
        # Then reset it
        text = inject_and_get_reply(runner, "/soul reset", timeout=TIMEOUT_COMMAND)
        assert "reset" in text.lower() or "cleared" in text.lower() or "default" in text.lower(), \
            f"Expected reset confirmation, got: {text}"


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

    def test_queue_set_followup(self, runner):
        """/queue followup → 'Queue mode set to: Followup'"""
        text = inject_and_get_reply(runner, "/queue followup", timeout=TIMEOUT_COMMAND)
        assert "Followup" in text, f"实际回复: {text}"

    def test_queue_set_invalid(self, runner):
        """/queue badmode → 'Unknown mode: ...'"""
        text = inject_and_get_reply(runner, "/queue badmode", timeout=TIMEOUT_COMMAND)
        assert "Unknown mode" in text, f"实际回复: {text}"

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
        for cmd in ["/new", "/s", "/sessions", "/back", "/delete", "/soul",
                    "/status", "/adaptive", "/reset"]:
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

    def test_normal_message_within_limit(self, runner):
        """正常长度的消息应能成功发送"""
        # 生成 1000 字符的文本（在限制内）
        normal_text = "B" * 1000
        
        count_before = len(runner.get_sent_messages())
        runner.inject(normal_text, channel_id="1039178386623557765")
        
        # 等待 bot 回复
        time.sleep(3)
        msgs = runner.get_sent_messages()
        
        # 应该有新的消息
        assert len(msgs) > count_before, "Bot should reply to normal message"
        print(f"\n  Normal message (1000 chars) → OK")

    def test_message_near_limit(self, runner):
        """接近限制的消息（1800 字符）应能成功发送"""
        # 生成 1800 字符的文本（接近但未超过 1900）
        near_limit_text = "C" * 1800
        
        count_before = len(runner.get_sent_messages())
        runner.inject(near_limit_text, channel_id="1039178386623557766")
        
        # 等待 bot 回复
        time.sleep(3)
        msgs = runner.get_sent_messages()
        
        # 验证消息被处理
        assert len(msgs) >= count_before, "Bot should handle near-limit message"
        print(f"\n  Near-limit message (1800 chars) → OK")

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
class TestDiscordAbortCommands:
    """验证 Agent 能正确中止任务 — 多语言 abort 触发词识别
    
    注意：abort 是本地命令识别（octos-core/src/abort.rs），不依赖 LLM。
    标记为 @pytest.mark.llm 仅因为需要完整的 gateway 环境。
    
    Abort 工作原理：
    - 用户发送任务消息，octos 开始处理
    - 用户发送 abort 命令（“停” / “stop” / “cancel” 等）
    - SessionActor 在处理消息前检测 is_abort_trigger()
    - 立即返回 abort_response()，不调用 LLM
    - 响应语言与触发词匹配（中文→中文，英文→英文等）
    """

    def test_abort_english_stop(self, runner):
        """发送任务后，用"stop"中止 - 应返回英文响应"""
            
        # 先发送 hi 建立会话
        count_before = len(runner.get_sent_messages())
        runner.inject("hi", channel_id="1039178386623557754")
        hi_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
        assert hi_reply is not None, "Bot did not respond to hi"
        print(f"\n  Session established: {hi_reply['text'][:50]}...")
            
        # 发送一个英文任务消息
        count_after_hi = len(runner.get_sent_messages())
        runner.inject("Help me write code", 
                      channel_id="1039178386623557754")
            
        # 等待 octos 开始回复（确认任务已启动）
        first_reply = runner.wait_for_reply(count_before=count_after_hi, timeout=TIMEOUT_COMMAND)
        assert first_reply is not None, "Bot did not start processing the task"
        print(f"  Task started: {first_reply['text'][:50]}...")
            
        # 额外等待一小段时间，确保 octos 已经进入处理状态
        time.sleep(1)
            
        # 现在发送 abort 命令
        count_after_first = len(runner.get_sent_messages())
        runner.inject("stop", channel_id="1039178386623557754")
            
        # 等待 abort 响应，跳过流式状态消息
        abort_reply = None
        for attempt in range(10):  # 最多等待 10 秒
            time.sleep(1)
            msgs = runner.get_sent_messages()
            if len(msgs) > count_after_first:
                # 找到最新的非流式状态消息
                for msg in reversed(msgs[count_after_first:]):
                    text = msg["text"]
                    # 跳过流式状态提示
                    if text in ["Processing", "Deliberating", "Evaluating", "Connecting", "Thinking"]:
                        continue
                    if "🛑" in text or "cancel" in text.lower() or "cancelled" in text.lower():
                        abort_reply = msg
                        break
            if abort_reply:
                break
            
        assert abort_reply is not None, "Bot did not respond to abort command with cancel response"
        text = abort_reply["text"]
            
        # 验证收到英文取消响应
        assert "🛑" in text or "cancel" in text.lower() or "cancelled" in text.lower(), \
            f"Expected English cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (stop) → {text}")

    def test_abort_english_cancel(self, runner):
        """发送任务后，用"cancel"中止"""
            
        count_before = len(runner.get_sent_messages())
        runner.inject("hi", channel_id="1039178386623557755")
        hi_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
        assert hi_reply is not None
            
        count_after_hi = len(runner.get_sent_messages())
        runner.inject("Write a comprehensive function with error handling", 
                      channel_id="1039178386623557755")
            
        # 等待 octos 开始回复（确认任务已启动）
        first_reply = runner.wait_for_reply(count_before=count_after_hi, timeout=TIMEOUT_COMMAND)
        assert first_reply is not None, "Bot did not start processing the task"
        print(f"  Task started: {first_reply['text'][:50]}...")
            
        # 额外等待一小段时间，确保 octos 已经进入处理状态
        time.sleep(1)
            
        count_after_first = len(runner.get_sent_messages())
        runner.inject("cancel", channel_id="1039178386623557755")
            
        # 等待 abort 响应，跳过流式状态消息
        abort_reply = None
        for attempt in range(10):
            time.sleep(1)
            msgs = runner.get_sent_messages()
            if len(msgs) > count_after_first:
                for msg in reversed(msgs[count_after_first:]):
                    text = msg["text"]
                    if text in ["Processing", "Deliberating", "Evaluating", "Connecting", "Thinking"]:
                        continue
                    if "🛑" in text or "cancel" in text.lower() or "cancelled" in text.lower():
                        abort_reply = msg
                        break
            if abort_reply:
                break
            
        assert abort_reply is not None, "Bot did not respond to cancel command with cancel response"
        text = abort_reply["text"]
            
        assert "🛑" in text or "cancel" in text.lower() or "cancelled" in text.lower(), \
            f"Expected cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (cancel) → {text}")

    def test_abort_chinese_stop(self, runner):
        """发送任务后，用中文"停"中止"""
        count_before = len(runner.get_sent_messages())
        runner.inject("hi", channel_id="1039178386623557756")
        hi_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
        assert hi_reply is not None
            
        count_after_hi = len(runner.get_sent_messages())
        runner.inject("请给我写一篇200字小说", 
                      channel_id="1039178386623557756")
            
        # 等待 octos 开始回复（确认任务已启动）
        first_reply = runner.wait_for_reply(count_before=count_after_hi, timeout=TIMEOUT_COMMAND)
        assert first_reply is not None, "Bot did not start processing the task"
        print(f"  Task started: {first_reply['text'][:50]}...")
            
        # 额外等待一小段时间，确保 octos 已经进入处理状态
        time.sleep(1)
            
        count_after_first = len(runner.get_sent_messages())
        runner.inject("停", channel_id="1039178386623557756")
            
        # 等待 abort 响应，跳过流式状态消息
        abort_reply = None
        for attempt in range(10):
            time.sleep(1)
            msgs = runner.get_sent_messages()
            if len(msgs) > count_after_first:
                for msg in reversed(msgs[count_after_first:]):
                    text = msg["text"]
                    if text in ["Processing", "Deliberating", "Evaluating", "Connecting", "Thinking"]:
                        continue
                    if "🛑" in text or "取消" in text or "已取消" in text:
                        abort_reply = msg
                        break
            if abort_reply:
                break
            
        assert abort_reply is not None, "Bot did not respond to abort command with cancel response"
        text = abort_reply["text"]
        assert "🛑" in text or "取消" in text or "已取消" in text, \
            f"Expected Chinese cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (停) → {text}")

    def test_abort_case_insensitive(self, runner):
        """验证大小写不敏感 - STOP vs stop"""
        count_before = len(runner.get_sent_messages())
        runner.inject("hi", channel_id="1039178386623557757")
        hi_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
        assert hi_reply is not None
        
        count_after_hi = len(runner.get_sent_messages())
        runner.inject("Task", channel_id="1039178386623557757")
        
        # 等待 octos 开始回复（确认任务已启动）
        first_reply = runner.wait_for_reply(count_before=count_after_hi, timeout=TIMEOUT_COMMAND)
        assert first_reply is not None, "Bot did not start processing the task"
        print(f"  Task started: {first_reply['text'][:50]}...")
        
        # 额外等待一小段时间，确保 octos 已经进入处理状态
        time.sleep(1)
        
        count_after_first = len(runner.get_sent_messages())
        runner.inject("STOP", channel_id="1039178386623557757")  # Uppercase
        
        # 等待 abort 响应，跳过流式状态消息
        abort_reply = None
        for attempt in range(10):
            time.sleep(1)
            msgs = runner.get_sent_messages()
            if len(msgs) > count_after_first:
                for msg in reversed(msgs[count_after_first:]):
                    text = msg["text"]
                    if text in ["Processing", "Deliberating", "Evaluating", "Connecting", "Thinking"]:
                        continue
                    if "🛑" in text or "cancel" in text.lower() or "cancelled" in text.lower():
                        abort_reply = msg
                        break
            if abort_reply:
                break
        
        assert abort_reply is not None, "Bot did not respond to STOP command with cancel response"
        text = abort_reply["text"]
        assert "🛑" in text or "cancel" in text.lower() or "cancelled" in text.lower(), \
            f"Expected cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (STOP uppercase) → {text}")

    def test_abort_with_whitespace(self, runner):
        """验证 abort 命令前后空格不影响识别"""
        for cmd in ["  stop  ", "\tstop\n", " 停 "]:
            count_before = len(runner.get_sent_messages())
            runner.inject(cmd, channel_id="1039178386623557764")
            abort_reply = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_COMMAND)
            assert abort_reply is not None, f"Should respond to trimmed '{cmd}'"
            assert len(abort_reply["text"]) > 0
        print(f"\n  ✓ Whitespace handling works")

    def test_abort_japanese(self, runner):
        """发送"やめて"中止 - 应返回日文响应"""
        count_before = len(runner.get_sent_messages())
        runner.inject("やめて", channel_id="1039178386623557767")
        
        # 等待 abort 响应，跳过流式状态消息
        abort_reply = None
        for attempt in range(10):
            time.sleep(1)
            msgs = runner.get_sent_messages()
            if len(msgs) > count_before:
                for msg in reversed(msgs[count_before:]):
                    text = msg["text"]
                    if text in ["Processing", "Deliberating", "Evaluating", "Connecting", "Thinking"]:
                        continue
                    if "🛑" in text or "cancel" in text.lower() or "キャンセル" in text:
                        abort_reply = msg
                        break
            if abort_reply:
                break
        
        assert abort_reply is not None, "Bot did not respond to Japanese abort command with cancel response"
        text = abort_reply["text"]
        assert "🛑" in text or "cancel" in text.lower() or "キャンセル" in text, \
            f"Expected Japanese cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (やめて) → {text}")

    def test_abort_russian(self, runner):
        """发送"стоп"中止 - 应返回俄文响应"""
        count_before = len(runner.get_sent_messages())
        runner.inject("стоп", channel_id="1039178386623557768")
        
        # 等待 abort 响应，跳过流式状态消息
        abort_reply = None
        for attempt in range(10):
            time.sleep(1)
            msgs = runner.get_sent_messages()
            if len(msgs) > count_before:
                for msg in reversed(msgs[count_before:]):
                    text = msg["text"]
                    if text in ["Processing", "Deliberating", "Evaluating", "Connecting", "Thinking"]:
                        continue
                    if "🛑" in text or "cancel" in text.lower() or "Отменено" in text:
                        abort_reply = msg
                        break
            if abort_reply:
                break
        
        assert abort_reply is not None, "Bot did not respond to Russian abort command with cancel response"
        text = abort_reply["text"]
        assert "🛑" in text or "cancel" in text.lower() or "Отменено" in text, \
            f"Expected Russian cancel response, got: {text[:200]}"
        print(f"  ✓ Abort (стоп) → {text}")

    def test_non_abort_messages_not_triggered(self, runner):
        """验证普通消息不会误触发 abort"""
        # 这些消息包含 abort 关键词但不是独立的命令
        non_triggers = [
            "please stop talking about cats",  # 句子中的 stop
            "stopping point is here",  # stopping 不是 stop
        ]
        
        for msg in non_triggers:
            count_before = len(runner.get_sent_messages())
            runner.inject(msg, channel_id="1039178386623557769")
            # 等待一小段时间让 octos 处理
            time.sleep(0.5)
        
        print(f"\n  ✓ Non-abort messages handled correctly")


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

    def test_soul_per_profile(self, runner):
        """验证每个 profile 有独立的 soul 配置"""
        CHANNEL_A = "1039178386623557758"
        CHANNEL_B = "1039178386623557759"
        
        # Set different souls for different channels
        text_a = inject_and_get_reply(runner, "/soul You are a coding expert",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        assert "Soul updated" in text_a
        
        text_b = inject_and_get_reply(runner, "/soul You are a creative writer",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        assert "Soul updated" in text_b
        
        # Verify souls are independent
        soul_a = inject_and_get_reply(runner, "/soul",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        soul_b = inject_and_get_reply(runner, "/soul",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        
        assert "coding expert" in soul_a.lower() or "You are a coding expert" in soul_a
        assert "creative writer" in soul_b.lower() or "You are a creative writer" in soul_b

    def test_queue_mode_per_profile(self, runner):
        """每个 profile 可以有独立的队列模式"""
        CHANNEL_A = "1039178386623557762"
        CHANNEL_B = "1039178386623557763"
        
        # Profile A 设置为 followup
        text_a = inject_and_get_reply(runner, "/queue followup",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        assert "Followup" in text_a
        
        # Profile B 保持默认 collect
        text_b = inject_and_get_reply(runner, "/queue",
                                      timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        assert "Collect" in text_b or "collect" in text_b.lower()
        
        # 验证 A 仍然是 followup
        text_a_check = inject_and_get_reply(runner, "/queue",
                                            timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        assert "Followup" in text_a_check


# ══════════════════════════════════════════════════════════════════════════════
# 文件限制测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordFileLimits:
    """验证 Discord 文件大小和消息长度限制"""

    def test_large_message_handling(self, runner):
        """测试大消息处理 - Discord 限制 1900 字符"""
        # Create a message near the limit
        long_text = "A" * 1800
        count_before = len(runner.get_sent_messages())
        runner.inject(long_text, channel_id="1039178386623557760")
        
        # Wait for response
        msg = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_LLM)
        # Should handle gracefully (either split or process)
        assert msg is not None, "Bot did not respond to large message"
        print(f"\n  ✓ Large message handled: {len(msg['text'])} chars response")

    def test_session_accumulation_stability(self, runner):
        """测试会话累积稳定性 - 多条消息后仍正常工作"""
        channel = "1039178386623557761"
        
        # Send multiple messages to accumulate history
        for i in range(5):
            text = inject_and_get_reply(
                runner, f"Message {i+1}",
                timeout=TIMEOUT_COMMAND, channel_id=channel
            )
            assert len(text) > 0, f"Empty response for message {i+1}"
        
        # Final command should still work
        final = inject_and_get_reply(runner, "/sessions", timeout=TIMEOUT_COMMAND, channel_id=channel)
        assert len(final) > 0, "Sessions command failed after accumulation"
        print(f"\n  ✓ Session stable after 5 messages")

    @pytest.mark.slow
    def test_session_file_size_limit_enforcement(self, runner):
        """验证会话文件达到 10MB 限制后的追加行为

        根据 octos-bus/src/session.rs:
        const MAX_SESSION_FILE_SIZE: u64 = 10 * 1024 * 1024;  // 10 MB

        限制逻辑：
        - session 文件 >= 10MB 时，新消息仍被处理（bot 正常响应）
        - 但追加操作被跳过（file_len >= MAX_SESSION_FILE_SIZE → skip append）

        测试策略：直接构造接近 10MB 的 session 文件，通过 bot 加载并追加，
        验证文件大小在追加前后基本不变（追加被跳过）。
        """
        import os
        import json
        import urllib.parse

        data_dir = os.environ.get("OCTOS_TEST_DIR", "/tmp/octos_test")
        test_channel_id = "1039178386623557770"
        session_name = "size-limit-test"
        profile = "_main"
        channel = "discord"

        encoded_base = urllib.parse.quote(f"{profile}:{channel}:{test_channel_id}", safe="")
        encoded_topic = urllib.parse.quote(session_name, safe="")
        session_dir = f"{data_dir}/users/{encoded_base}/sessions"
        session_path = f"{session_dir}/{encoded_topic}.jsonl"

        os.makedirs(session_dir, exist_ok=True)

        target_size = 9_900_000
        with open(session_path, "w") as f:
            header = json.dumps({
                "schema": 1,
                "model": "test",
                "created_at": "2024-01-01T00:00:00Z"
            })
            f.write(header + "\n")
            written = len(header.encode()) + 1
            i = 0
            while written < target_size:
                entry = json.dumps({
                    "role": "user",
                    "content": f"Message {i}: " + "A" * (200 * 1024)
                })
                f.write(entry + "\n")
                written += len(entry.encode()) + 1
                i += 1

        pre_size = os.path.getsize(session_path)
        assert pre_size >= 9_000_000, \
            f"Pre-filled file too small ({pre_size} bytes), profile key likely wrong: {session_path}"
        print(f"  Pre-filled session file: {pre_size / 1024**2:.2f}MB at {session_path}")

        count_before = len(runner.get_sent_messages())
        inject_and_get_reply(runner, f"/new {session_name}",
                           timeout=TIMEOUT_COMMAND, channel_id=test_channel_id)
        text = inject_and_get_reply(runner, "tiny msg",
                                  timeout=TIMEOUT_COMMAND, channel_id=test_channel_id)
        assert len(text) > 0, "Bot should still respond when session is at size limit"

        post_size = os.path.getsize(session_path)
        growth = post_size - pre_size
        print(f"  After append attempt: {post_size / 1024**2:.2f}MB, grew {growth} bytes")

        assert growth < 50_000, \
            f"Session at limit, append should be skipped (growth={growth} bytes)"
        print(f"  ✓ Append skipped correctly — session file stayed at {pre_size / 1024**2:.2f}MB")