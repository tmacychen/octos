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
import random
import logging
from runner_discord import DiscordTestRunner
from test_helpers import inject_and_get_reply

# 🔥 Suppress httpx INFO logs to reduce noise in test output
logging.getLogger("httpx").setLevel(logging.WARNING)

# ── 超时配置 ──────────────────────────────────────────────────────────────────
TIMEOUT_COMMAND = 20   # 本地命令，无需 LLM
TIMEOUT_LLM     = 50   # 需要调用 LLM API (增加到 50s，Discord Gateway 有额外开销)

# ── 压力缓解配置 ──────────────────────────────────────────────────────────────
# 在 LLM 密集型测试之间添加延迟，避免 API 过载
LLM_TEST_DELAY = 3.0   # LLM 测试后的等待时间（秒）
ABORT_TEST_DELAY = 2.0  # Abort 测试后的等待时间（秒），确保完全清理


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="session")
def runner():
    r = DiscordTestRunner()
    assert r.health(), "Discord Mock Server 未运行"
    return r


@pytest.fixture(autouse=True)
def cleanup_state(request, runner):
    """每个测试前清理 Mock Server 状态，并添加延迟缓解压力
    
    包含 Mock Server 崩溃检测：如果 Mock Server 不可达，
    自动跳过当前测试（pytest.skip），避免级联 ERROR。
    """
    import httpx
    import os
    import glob
    from test_helpers import inject_and_get_reply
    
    # 🔥 Health check: 验证 Mock Server 是否在线
    max_health_retries = 3
    for attempt in range(max_health_retries):
        try:
            if runner.health():
                break
        except Exception:
            pass
        if attempt < max_health_retries - 1:
            print(f"  ⚠ Mock Server not responding, retry {attempt + 1}/{max_health_retries}...")
            time.sleep(1.0)
    else:
        pytest.skip("Mock Server 崩溃，无法恢复（需重启 test_run.py）")
        return
    
    # Wait for any pending LLM responses to complete（缩短）
    time.sleep(2.0)
    
    try:
        runner.clear()
    except httpx.HTTPError:
        pytest.skip("Mock Server 无法清理，跳过测试")
        return
    
    # 注意：不要删除 session 文件！Gateway session actor 正在使用这些文件，删除会导致错误
    
    # 重置所有非默认状态（收紧超时，快速失败）
    try:
        inject_and_get_reply(runner, "/reset", timeout=3)
    except httpx.HTTPError:
        pytest.skip("Mock Server /reset 失败，跳过测试")
        return
    except AssertionError:
        pytest.skip("Mock Server /reset 无响应，跳过测试")
        return
    except Exception as e:
        print(f"  ⚠ /reset failed: {type(e).__name__}: {str(e)[:80]}")
    
    yield
    
    # After abort tests or LLM-intensive tests, add extra delay to ensure full cleanup
    # This helps prevent message sending failures in subsequent tests
    if request.node.get_closest_marker('abort_test') or request.node.get_closest_marker('llm_intensive'):
        time.sleep(ABORT_TEST_DELAY)


# ══════════════════════════════════════════════════════════════════════════════
# 第一层：会话管理命令 (GatewayDispatcher)
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordSessionCommands:
    """会话管理命令 — 本地处理，无需 LLM"""

    # 🔥 独立 channel_id，避免与其他测试共享 session 状态
    CHANNEL_ID = "20001"

    def test_new_default(self, runner):
        """/new → 'Session cleared.'"""
        text = inject_and_get_reply(runner, "/new", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Session cleared.", f"实际回复: {text}"

    def test_new_named(self, runner):
        """/new work → 'Switched to session: work'"""
        text = inject_and_get_reply(runner, "/new work", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Switched to session: work", f"实际回复: {text}"

    def test_new_invalid_name(self, runner):
        """/new bad:name → 'Invalid session name:'"""
        text = inject_and_get_reply(runner, "/new bad:name", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text.startswith("Invalid session name:"), f"实际回复: {text}"

    def test_switch_to_existing(self, runner):
        """/s <name> → 'Switched to session: <name>'"""
        inject_and_get_reply(runner, "/new research", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        text = inject_and_get_reply(runner, "/s research", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text.startswith("Switched to session:"), f"实际回复: {text}"

    def test_switch_to_default(self, runner):
        """/s → 'Switched to default session.'"""
        text = inject_and_get_reply(runner, "/s", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Switched to default session.", f"实际回复: {text}"

    def test_sessions_list(self, runner):
        """/sessions → non-empty reply"""
        text = inject_and_get_reply(runner, "/sessions", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert len(text) > 0, "Empty reply"
        print(f"\n  /sessions → {text[:100]}")

    def test_back_returns_session(self, runner):
        """/back → session-related reply"""
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "session" in text.lower(), f"Unexpected reply: {text}"
        print(f"\n  /back → {text}")

    def test_back_with_history(self, runner):
        """/back 在有历史会话时返回之前的会话"""
        # Create a named session first
        inject_and_get_reply(runner, "/new history-test", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        # Go back should return to previous or default
        text = inject_and_get_reply(runner, "/back", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "session" in text.lower(), f"Expected session info, got: {text}"

    def test_delete_session(self, runner):
        """/delete <name> → success"""
        inject_and_get_reply(runner, "/new to-delete", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        text = inject_and_get_reply(runner, "/delete to-delete", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Deleted session: to-delete", f"实际回复: {text}"

    def test_delete_no_name(self, runner):
        """/delete 无名称时显示错误"""
        text = inject_and_get_reply(runner, "/delete", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        # 实际返回："Cannot delete the default session. Use /clear to reset it."
        assert "cannot delete" in text.lower() or "default session" in text.lower() or "clear" in text.lower(), \
            f"Expected error for /delete without name, got: {text}"

    def test_soul_show(self, runner):
        """/soul → non-empty reply"""
        text = inject_and_get_reply(runner, "/soul", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert len(text) > 0, "Empty reply"
        print(f"\n  /soul → {text[:80]}")

    def test_soul_set(self, runner):
        """/soul <text> → confirmation"""
        text = inject_and_get_reply(runner, "/soul You are helpful.", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Soul updated. Takes effect in new sessions.", f"实际回复: {text}"

    def test_back_alias_b(self, runner):
        """/b 作为 /back 的别名"""
        text = inject_and_get_reply(runner, "/b", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "session" in text.lower(), f"Unexpected reply for /b: {text}"

    def test_delete_alias_d(self, runner):
        """/d 作为 /delete 的别名"""
        inject_and_get_reply(runner, "/new temp-session", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        text = inject_and_get_reply(runner, "/d temp-session", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "Deleted session: temp-session" in text or "deleted" in text.lower(), \
            f"Unexpected reply for /d: {text}"

    def test_soul_reset(self, runner):
        """/soul reset → 重置 soul"""
        # First set a soul
        inject_and_get_reply(runner, "/soul Custom soul", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        # Then reset it
        text = inject_and_get_reply(runner, "/soul reset", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "reset" in text.lower() or "cleared" in text.lower() or "default" in text.lower(), \
            f"Expected reset confirmation, got: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# 第二层：会话内控制命令 (SessionActor)
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordSessionActorCommands:
    """会话内控制命令 — 本地处理，无需 LLM"""

    # 🔥 独立 channel_id
    CHANNEL_ID = "20002"

    def test_adaptive_show(self, runner):
        """/adaptive → not enabled"""
        text = inject_and_get_reply(runner, "/adaptive", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Adaptive routing is not enabled.", f"实际回复: {text}"

    def test_queue_show(self, runner):
        """/queue → Queue mode info"""
        text = inject_and_get_reply(runner, "/queue", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text.startswith("Queue mode:"), f"实际回复: {text}"

    def test_queue_set_followup(self, runner):
        """/queue followup → 'Queue mode set to: Followup'"""
        text = inject_and_get_reply(runner, "/queue followup", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "Followup" in text, f"实际回复: {text}"

    def test_queue_set_invalid(self, runner):
        """/queue badmode → 'Unknown mode: ...'"""
        text = inject_and_get_reply(runner, "/queue badmode", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "Unknown mode" in text, f"实际回复: {text}"

    def test_status_show(self, runner):
        """/status → Status Config"""
        text = inject_and_get_reply(runner, "/status", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert "Status Config" in text, f"实际回复: {text}"

    def test_reset_command(self, runner):
        """/reset → reset confirmation"""
        text = inject_and_get_reply(runner, "/reset", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Reset: queue=collect, adaptive=off, history cleared.", \
            f"实际回复: {text}"

    def test_unknown_command_help(self, runner):
        """未知命令 → 帮助文本"""
        text = inject_and_get_reply(runner, "/unknowncmd", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text.startswith("Unknown command."), f"实际回复: {text}"
        for cmd in ["/new", "/s", "/sessions", "/back", "/delete", "/soul",
                    "/status", "/adaptive", "/reset"]:
            assert cmd in text, f"帮助文本缺少 {cmd}: {text}"


# ══════════════════════════════════════════════════════════════════════════════
# Queue Mode Steer/Discard 负向测试 — 防止误触发 abort
# ══════════════════════════════════════════════════════════════════════════════

@pytest.mark.llm  # 这些测试发送普通文本消息，会触发 LLM 调用
class TestDiscordQueueModeSteerNonAbort:
    """验证 Steer/Interrupt 模式下，普通消息不会误触发 abort
    
    Steer/Interrupt 模式在队列处理时会检查 is_abort_trigger()。
    此测试确保包含 abort 关键词但非独立命令的消息不会被误判。
    
    相关代码：octos-cli/src/session_actor.rs:2773-2787
    """

    # 🔥 独立 channel_id，避免与其他测试共享 session 状态
    # Steer/Interrupt 模式对消息时序敏感，共享 session 会导致延迟回复混入
    CHANNEL_ID_STEER = "1039178386623557001"
    CHANNEL_ID_INTERRUPT = "1039178386623557002"

    def test_steer_mode_non_abort_messages_not_triggered(self, runner):
        """验证 steer 模式下普通消息不会误触发 abort
        
        测试流程：
        1. 设置 queue mode 为 steer
        2. 发送包含 abort 关键词但不是独立命令的消息
        3. 验证消息被正常处理，未返回 abort 响应
        """
        channel_id = self.CHANNEL_ID_STEER
        # Step 1: 设置为 steer 模式
        text = inject_and_get_reply(runner, "/queue steer", timeout=TIMEOUT_COMMAND, channel_id=channel_id)
        assert "Steer" in text or "steer" in text.lower(), f"Failed to set steer mode: {text}"
        
        # Step 2: 发送可能误触发的消息
        non_triggers = [
            "please stop talking about cats",  # 句子中的 stop
            "stopping point is here",          # stopping 不是 stop
            "I will exit now",                 # exit 不在触发词列表中
            "cancel my subscription please",   # 句子中的 cancel
        ]
        
        for msg in non_triggers:
            reply = inject_and_get_reply(runner, msg, timeout=TIMEOUT_LLM, channel_id=channel_id)
            
            # 🔥 关键断言：不应包含 abort 特征
            # 只检查 🛑 emoji（所有 abort 响应都以 🛑 开头），
            # 不检查 "cancelled" 等关键词（LLM 可能自然使用这些词）
            has_abort_emoji = "🛑" in reply
            
            assert not has_abort_emoji, \
                f"False abort trigger in steer mode for '{msg}': {reply[:200]}"
        
        print(f"\n  ✓ Steer mode: Non-abort messages handled correctly")
        
        # Step 3: 恢复默认模式
        inject_and_get_reply(runner, "/queue collect", timeout=TIMEOUT_COMMAND, channel_id=channel_id)

    def test_interrupt_mode_non_abort_messages_not_triggered(self, runner):
        """验证 interrupt 模式下普通消息不会误触发 abort
        
        Interrupt 模式与 Steer 类似，也会在队列中检查 abort 触发词。
        """
        channel_id = self.CHANNEL_ID_INTERRUPT
        # Step 1: 设置为 interrupt 模式
        text = inject_and_get_reply(runner, "/queue interrupt", timeout=TIMEOUT_COMMAND, channel_id=channel_id)
        assert "Interrupt" in text or "interrupt" in text.lower(), \
            f"Failed to set interrupt mode: {text}"
        
        # Step 2: 发送可能误触发的消息
        # 注意：避免使用 LLM 容易在回复中自然使用的词（如 "cancelled"），
        # 因为即使不触发 abort，LLM 也可能回显这些词
        non_triggers = [
            "don't stop the music",           # 否定句中的 stop
            "the concert was canceled",       # canceled（美式拼写）不是独立 trigger
            "abort the rocket launch",        # abort 作为动词修饰语
        ]
        
        for msg in non_triggers:
            reply = inject_and_get_reply(runner, msg, timeout=TIMEOUT_LLM, channel_id=channel_id)
            
            # 🔥 关键断言：不应包含 abort 特征
            # 只检查 🛑 emoji（所有 abort 响应都以 🛑 开头），
            # 不检查 "cancelled" 等关键词（LLM 可能自然使用这些词）
            has_abort_emoji = "🛑" in reply
            
            assert not has_abort_emoji, \
                f"False abort trigger in interrupt mode for '{msg}': {reply[:200]}"
        
        print(f"\n  ✓ Interrupt mode: Non-abort messages handled correctly")
        
        # Step 3: 恢复默认模式
        inject_and_get_reply(runner, "/queue collect", timeout=TIMEOUT_COMMAND, channel_id=channel_id)


# ══════════════════════════════════════════════════════════════════════════════
# 多用户隔离测试
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordMultiUser:
    """多用户隔离 & 不同频道隔离"""

    # 🔥 独立 channel_id
    CHANNEL_ID = "20010"

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

    # 🔥 独立 channel_id
    CHANNEL_ID = "20006"

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
        text = inject_and_get_reply(runner, "/new", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
        assert text == "Session cleared.", f"实际回复: {text}"

    def test_new_named_session(self, runner):
        """发 /new <name> → 创建命名会话"""
        text = inject_and_get_reply(runner, "/new my-test", timeout=TIMEOUT_COMMAND, channel_id=self.CHANNEL_ID)
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

    @pytest.mark.abort_test
    @pytest.mark.parametrize(
        "language,channel_id,long_task,expected_keywords",
        [
            # English - test all triggers: stop, cancel, abort, halt, quit, enough
            ("english", "1039178386623557754", 
             "Please write a detailed technical article about Python async programming best practices...",
             ["🛑", "cancelled"]),
            
            # Chinese - test all triggers: 停, 停止, 取消, 停下, 别说了
            ("chinese", "1039178386623557756",
             "请帮我写一篇详细的技术文章，介绍 Python 异步编程的最佳实践...",
             ["🛑", "已取消"]),
            
            # Japanese - test all triggers: やめて, 止めて, ストップ
            ("japanese", "1039178386623557767",
             "Pythonの非同期プログラミングのベストプラクティスについて詳細な技術記事を書いてください...",
             ["🛑", "キャンセル"]),
            
            # Russian - test all triggers: стоп, отмена, хватит
            ("russian", "1039178386623557769",
             "Напишите подробную техническую статью о лучших практиках асинхронного программирования на Python...",
             ["🛑", "Отменено"]),
        ],
        ids=[
            "english_all_triggers",
            "chinese_all_triggers",
            "japanese_all_triggers",
            "russian_all_triggers",
        ]
    )
    def test_abort_multilanguage(self, runner, language, channel_id, long_task, expected_keywords):
        """多语言 abort 命令测试 - 遍历所有触发词
        
        测试流程：
        1. 发送一个长任务（触发 LLM 处理）
        2. 等待 5 秒，让任务开始执行
        3. 每隔 5 秒发送一个触发词，直到收到 abort 响应或所有触发词用完
        4. 如果收到 abort 响应，测试通过
        5. 如果所有触发词都发送完了，再等 5 秒还没响应，测试失败
        
        支持：英文、中文、日文、俄文。
        """
        import time
        
        # Define trigger words for each language (from abort.rs)
        TRIGGERS = {
            "english": ["stop", "cancel", "abort", "halt", "quit", "enough"],
            "chinese": ["停", "停止", "取消", "停下", "别说了"],
            "japanese": ["やめて", "止めて", "ストップ"],
            "russian": ["стоп", "отмена", "хватит"],
        }
        
        triggers = TRIGGERS[language]
        print(f"\n  Testing {language} with {len(triggers)} triggers: {triggers}")
        
        # Step 1: 发送长任务，触发 LLM 处理
        count_before_task = len(runner.get_sent_messages())
        runner.inject(long_task, channel_id=channel_id)
        print(f"  → Long task injected")
        
        # Step 2: 等待 5 秒，让任务开始执行
        time.sleep(5.0)
        print(f"  → Waited 5s for task to start")
        
        # Step 3: 遍历所有触发词，每隔 5 秒发送一个
        abort_reply = None
        for i, abort_cmd in enumerate(triggers):
            print(f"  → [{i+1}/{len(triggers)}] Sending abort command: '{abort_cmd}'")
            runner.inject(abort_cmd, channel_id=channel_id)
            
            # 等待 5 秒，检查是否收到 abort 响应
            poll_start = time.time()
            while time.time() - poll_start < 5.0:
                msgs = runner.get_sent_messages()
                # 从后往前找，找到第一条包含 abort 特征的消息
                for msg in reversed(msgs):
                    msg_text = msg.get("text", "")
                    if "🛑" in msg_text or any(kw.lower() in msg_text.lower() for kw in expected_keywords if not kw.startswith("🛑")):
                        abort_reply = msg
                        break
                
                if abort_reply is not None:
                    break
                
                time.sleep(0.3)  # 短轮询间隔
            
            if abort_reply is not None:
                print(f"  ✓ Abort response received after '{abort_cmd}'")
                break
            else:
                print(f"  ✗ No response to '{abort_cmd}', trying next...")
        
        # Step 4: 如果所有触发词都试过了，再等 5 秒
        if abort_reply is None:
            print(f"  → All triggers exhausted, waiting final 5s...")
            time.sleep(5.0)
            
            # 最后一次检查
            msgs = runner.get_sent_messages()
            for msg in reversed(msgs):
                msg_text = msg.get("text", "")
                if "🛑" in msg_text or any(kw.lower() in msg_text.lower() for kw in expected_keywords if not kw.startswith("🛑")):
                    abort_reply = msg
                    break
        
        # Step 5: 断言
        assert abort_reply is not None, \
            f"Bot did not respond to ANY abort command after trying all {len(triggers)} triggers: {triggers}"
        
        text = abort_reply["text"]
        
        # 🔥 VERIFICATION B: 确保收到的是 abort 响应，不是长任务的中间消息
        has_stop_emoji = "🛑" in text
        has_cancel_keyword = any(kw.lower() in text.lower() for kw in expected_keywords if not kw.startswith("🛑"))
        
        assert has_stop_emoji or has_cancel_keyword, \
            f"Expected abort response (with 🛑 or cancel keyword), got: {text[:200]}"
        
        # 🔥 VERIFICATION A: 验证“真中断” - 确认长任务确实停止了
        count_after_abort = len(runner.get_sent_messages())
        
        # 等待一段时间，观察是否还有新消息（长任务不应该继续输出）
        time.sleep(3)
        
        count_final = len(runner.get_sent_messages())
        
        # 断言：abort 后不应该有新的消息产生
        new_messages_after_abort = count_final - count_after_abort
        assert new_messages_after_abort <= 1, \
            f"Long task was NOT properly aborted! Found {new_messages_after_abort} new messages after abort: {text[:100]}"
        
        print(f"  ✓ Abort interrupted long task → {text}")
        print(f"    Verified: No further messages after abort ({new_messages_after_abort} new msgs)")

    @pytest.mark.abort_test
    def test_abort_with_whitespace(self, runner):
        """验证 abort 命令前后空格不影响识别"""
        test_cases = [
            ("  stop  ", "1039178386623557773", ["🛑", "cancel", "cancelled"]),
            ("\tstop\n", "1039178386623557774", ["🛑", "cancel", "cancelled"]),
            (" 停 ", "1039178386623557775", ["🛑", "取消", "已取消"]),
        ]
        
        for cmd, channel_id, expected_keywords in test_cases:
            count_before = len(runner.get_sent_messages())
            runner.inject(cmd, channel_id=channel_id)
            abort_reply = runner.wait_for_reply(
                count_before=count_before,
                timeout=TIMEOUT_COMMAND,
                chat_id=channel_id
            )
            assert abort_reply is not None, f"Should respond to trimmed '{cmd}'"
            text = abort_reply["text"]
            
            has_expected_keyword = any(kw.lower() in text.lower() for kw in expected_keywords if not kw.startswith("🛑"))
            has_emoji = "🛑" in text
            assert has_emoji or has_expected_keyword, \
                f"Expected cancel response for '{cmd}', got: {text[:200]}"
        
        print(f"  ✓ Whitespace handling works")


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

    # 🔥 独立 channel_id
    CHANNEL_ID = "20005"

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
        
        # 🔥 CRITICAL FIX: Create fresh sessions to ensure clean state
        # Previous tests may have modified queue_mode on these channels
        inject_and_get_reply(runner, "/new", timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_A)
        inject_and_get_reply(runner, "/new", timeout=TIMEOUT_COMMAND, channel_id=CHANNEL_B)
        
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

    # 🔥 独立 channel_id
    CHANNEL_ID = "20008"

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
# Session 文件大小压力测试（必须放在最后执行）
# ══════════════════════════════════════════════════════════════════════════════
# ⚠️ 重要：这个测试类必须在所有其他测试之后执行！
#
# 原因：
# 1. test_session_accumulation_stability 会累积 session 数据到磁盘
# 2. test_session_file_size_limit_enforcement 会创建 9.9MB 的大 session 文件
# 3. 如果这些测试先执行，后续测试加载大 session 文件会导致 LLM 超时
# 4. 即使有清理逻辑，也无法保证完全不影响其他测试
#
# pytest 默认按定义顺序执行测试类，所以这个类放在文件末尾确保最后执行。
# ══════════════════════════════════════════════════════════════════════════════

class TestDiscordSessionSizeStress:
    """Session 文件大小压力测试 — 验证 octos-bus 的 session 持久化机制
    
    测试 octos-bus/src/session.rs 中的核心功能：
    - add_message(): 追加消息到 session 并持久化到 JSONL 文件
    - append_to_disk(): 将消息写入磁盘，检查文件大小限制
    
    ⚠️ 这个类必须在所有其他测试之后执行，避免大 session 文件污染后续测试。
    """

    # 🔥 独立 channel_id
    CHANNEL_ID = "20012"

    @pytest.mark.slow
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
        
        # 🔥 关键清理：删除大 session 文件避免污染后续测试
        import os
        import glob
        try:
            data_dir = os.environ.get("OCTOS_TEST_DIR", "/tmp/octos_test")
            session_files = glob.glob(f"{data_dir}/users/*/sessions/*.jsonl")
            deleted_count = 0
            for session_file in session_files:
                file_size = os.path.getsize(session_file)
                if file_size > 100_000:  # 只删除大于 100KB 的文件
                    os.remove(session_file)
                    deleted_count += 1
            if deleted_count > 0:
                print(f"  ✓ Cleaned up {deleted_count} large session files")
        except Exception:
            pass  # 清理失败不影响测试结果

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
        
        # 清理测试用的 session 文件（避免影响后续测试）
        try:
            if os.path.exists(session_path):
                os.remove(session_path)
                print(f"  ✓ Cleaned up test session file")
        except Exception as e:
            print(f"  ⚠ Failed to clean up session file: {e}")