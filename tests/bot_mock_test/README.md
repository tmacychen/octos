# Bot Mock 测试框架

统一入口脚本 `run_test.fish` 管理 Telegram 和 Discord 的 mock 测试，无需真实的 API 凭证。

## 快速开始

```fish
# 查看帮助
fish tests/bot_mock/run_test.fish -h

# 运行指定模块
fish tests/bot_mock/run_test.fish telegram    # 或 tg
fish tests/bot_mock/run_test.fish discord     # 或 dc
fish tests/bot_mock/run_test.fish all         # 运行全部

# 列出模块 / 用例
fish tests/bot_mock/run_test.fish list
fish tests/bot_mock/run_test.fish cases tg
fish tests/bot_mock/run_test.fish cases discord
```

### 命令一览

| 命令 | 说明 |
|------|------|
| (无参数) / `-h` / `--help` | 显示帮助 |
| `telegram` / `tg` | 运行 Telegram 测试 |
| `discord` / `dc` | 运行 Discord 测试 |
| `all` | 运行所有模块 |
| `list` | 列出可用模块 |
| `cases <mod>` | 列出指定模块的测试用例 |

### 环境变量

| 变量 | 必需 | 说明 |
|------|------|------|
| `ANTHROPIC_API_KEY` | ✅ | LLM API 密钥 |
| `TELEGRAM_BOT_TOKEN` | Telegram 测试 | Telegram bot token |
| `DISCORD_BOT_TOKEN` | ❌ (自动填充) | Discord mock 模式自动设为假 token |

---

## 文件结构

```
tests/bot_mock/
├── run_test.fish           # 统一测试入口（支持命令行参数）
├── run_discord_test.fish   # Discord 独立测试入口（向后兼容）
├── mock_tg.py              # Mock Telegram API 服务器 (HTTP REST)
├── mock_discord.py         # Mock Discord 服务器 (REST + WebSocket Gateway)
├── runner.py               # BotTestRunner: Telegram pytest 辅助工具
├── runner_discord.py       # DiscordTestRunner: Discord pytest 辅助工具
├── test_telegram.py        # Telegram 测试用例 (45 个)
├── test_discord.py         # Discord 测试用例 (18 个)
├── requirements.txt
├── __init__.py
└── README.md
```

---

## Telegram 测试

### 架构图

```
┌─────────────────────────────────────────────────────────┐
│                      本地测试环境                         │
│                                                         │
│  ┌─────────────┐   GetUpdates (长轮询)  ┌─────────────┐ │
│  │ Mock Server │ ◄────────────────────  │  octos bot  │ │
│  │ (Python)    │  updates[]            │  (Rust)     │ │
│  │ 端口 5000   │  sendMessage          │ teloxide    │ │
│  └─────────────┘                        └─────────────┘ │
│         ▲                                               │
│         │  /_inject  /_sent_messages  /_clear           │
│  ┌─────────────┐                                        │
│  │ test_telegram.py │  (pytest)                               │
│  └─────────────┘                                        │
└─────────────────────────────────────────────────────────┘
```

**关键机制：** `TELOXIDE_API_URL=http://127.0.0.1:5000` 重定向所有 Telegram HTTP API 到 Mock Server。

### 超时配置
- 命令类: `TIMEOUT_COMMAND = 15s`
- LLM 消息: `TIMEOUT_LLM = 30s`

### BotTestRunner API

| 方法 | 说明 |
|------|------|
| `runner.inject(text, chat_id, username, is_group)` | 注入用户消息 |
| `runner.inject_callback(data, chat_id, message_id)` | 注入按钮回调 |
| `runner.wait_for_reply(count_before, timeout)` | 等待新消息，返回最新一条 |
| `runner.get_sent_messages()` | 获取所有已发消息列表 |
| `runner.clear()` | 清空消息记录 |

---

## Discord 测试

### 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                        本地测试环境                              │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐ │
│  │              Discord Mock Server                          │ │
│  │              (Python FastAPI + WebSocket)                  │ │
│  │              Port 5001                                    │ │
│  │                                                           │ │
│  │  ┌─────────────┐  WS /  (Gateway)   ┌─────────────┐     │ │
│  │  │ serenity    │◄══════════════════►│ Hello/Ready │     │ │
│  │  │ Client      │                     MESSAGE_CREATE│     │ │
│  │  │ (octos bot) │                     dispatch     │     │ │
│  │  └──────┬──────┘                     (injected)   │     │ │
│  │         │ POST /api/v10/channels/{id}/messages        │     │ │
│  │         ├────────────────────────────► send/reply    │     │ │
│  │         │ PUT .../messages/{mid}     edit            │     │ │
│  │         │ DELETE .../messages/{mid}  delete          │     │ │
│  │         │ GET /api/v10/gateway/bot → returns ws URL   │     │ │
│  │         │ GET /users/@me           → bot info         │     │ │
│  │  ─────────────────────────────────────────────────    │     │ │
│  │  │ POST /_inject                → inject user msg    │     │ │
│  │  │ POST /_inject_interaction     → inject interaction │     │ │
│  │  │ GET  /_sent_messages          → read bot replies  │     │ │
│  │  │ POST /_clear                 → reset state        │     │ │
│  │  └────────────────────────────────────────────────    │     │ │
│  └───────────────────────────────────────────────────────────┘ │
│                                                                 │
│  ┌───────────────────┐                                       │
│  │  test_discord.py   │  (pytest)                             │
│  │  runner_discord.py │  (DiscordTestRunner)                   │
│  └───────────────────┘                                       │
└─────────────────────────────────────────────────────────────────┘
```

**关键机制：**
1. `DISCORD_API_BASE_URL` 通过 serenity 的 `HttpBuilder::proxy()` 重定向 REST API 到 Mock Server
2. Mock Server 的 `/api/v10/gateway/bot` 返回本地 WS URL，serenity 自动连接
3. 无需真实的 Discord bot token — Mock Server 不校验 token

> **Rust 端改动** (`crates/octos-bus/src/discord_channel.rs`)：当 `DISCORD_API_BASE_URL` 环境变量存在时，使用 `HttpBuilder::proxy()` + `.ratelimiter_disabled(true)` 构建 Http，并通过 `ClientBuilder::new_with_http()` 传入自定义 Http 实例，确保 Client 内部也走代理。

### 超时配置
- 命令类: `TIMEOUT_COMMAND = 20s`
- LLM 消息: `TIMEOUT_LLM = 40s`
（比 Telegram 稍长，因为 Discord Gateway 有握手和心跳开销）

### DiscordTestRunner API

| 方法 | 说明 |
|------|------|
| `runner.inject(text, channel_id, sender_id, username, guild_id)` | 注入用户消息（分发为 MESSAGE_CREATE） |
| `runner.inject_interaction(data, channel_id, message_id)` | 注入交互事件（斜杠命令/按钮回调） |
| `runner.wait_for_reply(count_before, timeout)` | 等待新消息（每 0.5s 轮询） |
| `runner.get_sent_messages()` | 获取所有已发消息列表 |
| `runner.clear()` | 清空消息记录 |
| `runner.health()` | 检查 Mock Server 在线状态（含 WS 连接数）|

---

## 测试用例一览

### Telegram (26 个)

```
TestSessionCommands                  # 会话管理命令
  1  test_new_default                /new → Session cleared.
  2  test_new_named                  /new work → Switched to session: work
  3  test_new_invalid_name           /new bad:name → Invalid session name
  4  test_switch_to_existing         /s <name> → Switched
  5  test_switch_to_default          /s → Switched to default
  6  test_sessions_list              /sessions → non-empty
  7  test_back_returns_session       /back → session reply
  8  test_back_with_history          /back → Switched back
  9  test_back_alias_b               /b == /back
 10  test_delete_session             /delete <name> → Deleted
 11  test_delete_alias_d             /d == /delete
 12  test_delete_no_name             /delete → help
 13  test_soul_show_default          /soul → non-empty
 14  test_soul_set                   /soul <text> → updated
 15  test_soul_reset                 /soul reset → reset

TestSessionActorCommands             # 会话内控制命令
 16  test_adaptive_no_router         /adaptive → not enabled
 17  test_queue_show                 /queue → Queue mode
 18  test_queue_set_followup         /queue followup → Followup
 19  test_queue_set_invalid          /queue badmode → Unknown mode
 20  test_status_show                /status → Status Config
 21  test_reset_command              /reset → reset confirmation
 22  test_unknown_command_help       /unknowncmd → help text

TestMultiUser                        # 多用户隔离
 23  test_two_users_independent      different chat_id → isolated sessions
 24  test_callback_session_switch    inline keyboard callback → session switch

TestLLMMessages                      # LLM 消息 (需 ANTHROPIC_API_KEY)
 25  test_regular_message            Hello! → non-empty reply
 26  test_chinese_message            你好 → non-empty reply
```

### Discord (18 个)

```
TestDiscordSessionCommands           # 会话管理命令
  1  test_new_default                /new → Session cleared.
  2  test_new_named                  /new work → Switched to session: work
  3  test_new_invalid_name           /new bad:name → Invalid session name
  4  test_switch_to_existing         /s <name> → Switched
  5  test_switch_to_default          /s → Switched to default
  6  test_sessions_list              /sessions → non-empty
  7  test_back_returns_session       /back → session reply
  8  test_delete_session             /delete <name> → Deleted
  9  test_soul_show                  /soul → non-empty
 10  test_soul_set                   /soul <text> → updated

TestDiscordSessionActorCommands      # 会话内控制命令
 11  test_adaptive_show              /adaptive → not enabled
 12  test_queue_show                 /queue → Queue mode
 13  test_status_show                /status → Status Config
 14  test_reset_command              /reset → reset confirmation
 15  test_unknown_command_help       /unknowncmd → help text

TestDiscordMultiUser                 # 多频道隔离
 16  test_two_channels_independent   different channel_id → isolated sessions

TestDiscordLLMMessages               # LLM 消息 (需 ANTHROPIC_API_KEY)
 17  test_regular_message            Hello! → non-empty reply
 18  test_chinese_message            你好 → non-empty reply
```

---

## 添加新测试用例

### 步骤

1. 打开对应的测试文件（Telegram: `test_telegram.py`, Discord: `test_discord.py`）
2. 在合适的 class 里添加 `test_` 开头的方法
3. 使用 runner 的 `inject()` 发送消息
4. 使用 `wait_for_reply()` 等待回复
5. 用 `assert` 验证结果

### Telegram 测试模板

```python
def test_my_feature(self, runner: BotTestRunner):
    """描述这个测试验证什么"""
    runner.inject("/mycommand", chat_id=123)
    msg = runner.wait_for_reply(count_before=0, timeout=TIMEOUT_COMMAND)
    assert msg is not None, "Bot 未回复"
    assert "expected text" in msg["text"], f"回复内容不符: {msg['text']}"
```

### Discord 测试模板

```python
def test_my_feature(self, runner: DiscordTestRunner):
    """描述这个测试验证什么"""
    runner.inject("/mycommand", channel_id="1039178386623557754")
    msg = runner.wait_for_reply(count_before=0, timeout=TIMEOUT_COMMAND)
    assert msg is not None, "Bot 未回复"
    assert "expected text" in msg["text"], f"回复内容不符: {msg['text']}"
```

### 多轮对话示例（Discord）

```python
def test_multi_turn(self, runner: DiscordTestRunner):
    """测试多轮对话"""
    # 第一轮
    count_before = len(runner.get_sent_messages())
    runner.inject("你好")
    msg1 = runner.wait_for_reply(count_before=count_before, timeout=TIMEOUT_LLM)
    assert msg1 is not None

    # 第二轮（更新 count_before）
    count = len(runner.get_sent_messages())
    runner.inject("继续")
    msg2 = runner.wait_for_reply(count_before=count, timeout=TIMEOUT_LLM)
    assert msg2 is not None
```

### 添加新模块

1. 创建 `mock_<platform>.py`（Mock Server）和 `runner_<platform>.py`（TestRunner）
2. 创建 `test_<platform>.py`（pytest 测试用例）
3. 在 `run_test.fish` 的 `MODULES` 数组添加一行：`"name|alias|port|test_file|feature|mock_module|mock_class"`
4. 添加 `setup_<platform>_env` 函数
5. 更新 `__init__.py` 和 `requirements.txt`

---

## 调试技巧

```fish
# ====== Telegram 调试 ======
cat /tmp/octos_bot_test.log

# 手动启动 Telegram Mock Server
PYTHONPATH=tests/bot_mock \
  tests/bot_mock/.venv/bin/python -c "
import time; from mock_tg import MockTelegramServer
server = MockTelegramServer()
server.start_background()
print('Mock server at http://127.0.0.1:5000')
while True: time.sleep(1)
"

# 手动注入 Telegram 消息
curl -X POST http://127.0.0.1:5000/_inject \
  -H 'Content-Type: application/json' \
  -d '{"text": "/start", "chat_id": 123}'

# 查看 bot 回复
curl http://127.0.0.1:5000/_sent_messages


# ====== Discord 调试 ======
cat /tmp/octos_discord_bot_test.log

# 手动启动 Discord Mock Server
PYTHONPATH=tests/bot_mock \
  tests/bot_mock/.venv/bin/python -c "
import time; from mock_discord import MockDiscordServer
server = MockDiscordServer(port=5001)
server.start_background()
print('Mock Discord server at http://127.0.0.1:5001')
print('WebSocket at same address (ws://127.0.0.1:5001)')
while True: time.sleep(1)
"

# 手动注入 Discord 消息
curl -X POST http://127.0.0.1:5001/_inject \
  -H 'Content-Type: application/json' \
  -d '{"text": "/new", "channel_id": "1039178386623557754"}'

# 查看 Discord bot 回复
curl http://127.0.0.1:5001/_sent_messages

# 查看健康状态（包含 WS 连接数）
curl http://127.0.0.1:5001/health
```

## 已知限制

- LLM 测试受网络延迟影响，超时设置需留余量
- 媒体文件（图片/语音）Mock 返回假数据，不测试实际媒体处理
- 每次运行 bot 全新启动，不保留上次会话状态
- Discord Mock 不模拟 rate limit、权限检查等高级特性
- Discord Gateway 心跳为固定实现，不做超时断连测试
