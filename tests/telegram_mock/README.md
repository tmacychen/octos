# Telegram Mock 测试框架

## 整体架构

```
tests/telegram_mock/
├── run_test.fish   # 一键测试入口：启动环境 → 编译 → 运行 pytest → 清理
├── mock_tg.py      # Mock Telegram API 服务器
├── runner.py       # BotTestRunner：pytest 用的测试辅助工具类
├── test_bot.py     # 所有测试用例（pytest）
├── requirements.txt
└── README.md
```

## 测试原理

```
┌─────────────────────────────────────────────────────────┐
│                      本地测试环境                         │
│                                                         │
│  ┌─────────────┐   GetUpdates (长轮询)  ┌─────────────┐ │
│  │ Mock Server │ ◄────────────────────  │  octos bot  │ │
│  │ (Python)    │  updates[]            │  (Rust)     │ │
│  │             │ ─────────────────────► │             │ │
│  │ 模拟        │                        │ 处理消息    │ │
│  │ Telegram API│  sendMessage          │ 调用 LLM   │ │
│  │             │ ◄────────────────────  │             │ │
│  └─────────────┘                        └─────────────┘ │
│         ▲                                               │
│         │  /_inject  /_sent_messages  /_clear           │
│  ┌─────────────┐                                        │
│  │  test_bot.py│  (pytest 测试用例)                      │
│  │  runner.py  │  (BotTestRunner 工具类)                 │
│  └─────────────┘                                        │
└─────────────────────────────────────────────────────────┘
```

**关键机制：**

1. octos bot 通过环境变量 `TELOXIDE_API_URL=http://127.0.0.1:5000` 将所有 Telegram API 请求重定向到 Mock Server，无需真实 Telegram 网络连接
2. Mock Server 实现了 `GetUpdates`（长轮询）、`sendMessage` 等 Telegram API 端点
3. 测试用例通过 `/_inject` 注入模拟用户消息，通过 `/_sent_messages` 读取 bot 的回复，通过 `/_clear` 重置状态

## 所需资源

| 资源 | 说明 |
|------|------|
| `ANTHROPIC_API_KEY` | LLM API key（bot 调用 LLM 需要） |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token（格式验证用，不会真正连接 Telegram） |
| Python 3.11+ | Mock Server 运行环境 |
| `uv` | Python 包管理器（`brew install uv`） |
| Rust / Cargo | 编译 octos bot |

## 快速开始

```fish
# 设置环境变量
set -x ANTHROPIC_API_KEY "your-api-key"
set -x TELEGRAM_BOT_TOKEN "123456:ABC..."

# 从项目根目录运行
fish tests/telegram_mock/run_test.fish
```

`run_test.fish` 自动完成以下步骤：

1. 检查环境变量
2. 创建 Python venv 并安装依赖（首次运行）
3. 写入测试专用 config（`.octos/test_config.json`）
4. 清理并启动 Mock Server（端口 5000）
5. 编译 octos（`--features telegram`）
6. 启动 octos gateway，等待 "Gateway ready"
7. 调用 `pytest test_bot.py -v`
8. 清理所有进程，输出结果

## 当前测试用例

测试用例全部在 `test_bot.py` 中，按功能分为四组：

### TestSessionCommands — 会话管理命令（GatewayDispatcher）

本地处理，无需 LLM，超时 `TIMEOUT_COMMAND = 10s`。

| 测试方法 | 输入 | 验证 |
|----------|------|------|
| `test_new_session_default` | `/new` | bot 有回复 |
| `test_new_session_named` | `/new work` | 回复包含会话名 |
| `test_switch_session` | `/s research` | 切换成功 |
| `test_sessions_list` | `/sessions` | bot 有回复 |
| `test_back_command` | `/back` | bot 有回复 |
| `test_back_alias_b` | `/b` | 与 /back 等价 |
| `test_delete_session` | `/delete temp-session` | 删除成功 |
| `test_delete_alias_d` | `/d alias-test` | 与 /delete 等价 |
| `test_soul_show` | `/soul` | 查看 persona |
| `test_soul_set_and_reset` | `/soul <text>` + `/soul reset` | 设置并重置 |

### TestSessionActorCommands — 会话内控制命令（SessionActor）

本地处理，无需 LLM，超时 `TIMEOUT_COMMAND = 10s`。

| 测试方法 | 输入 | 验证 |
|----------|------|------|
| `test_adaptive_show` | `/adaptive` | bot 有回复 |
| `test_queue_show` | `/queue` | bot 有回复 |
| `test_status_show` | `/status` | bot 有回复 |
| `test_reset_command` | `/reset` | bot 有回复 |
| `test_unknown_command_shows_help` | `/unknowncmd` | 回复包含 /new 和 /sessions |

### TestMultiUser — 多用户隔离测试

| 测试方法 | 说明 | 验证 |
|----------|------|------|
| `test_two_users_independent_sessions` | 两个 chat_id 各自创建会话 | 两个用户都收到回复 |
| `test_callback_query` | 点击内联键盘按钮 | 回调被正常处理 |

### TestLLMMessages — LLM 消息测试（标记 `@pytest.mark.llm`）

需要调用 LLM API，超时 `TIMEOUT_LLM = 30s`。可用 `pytest -m "not llm"` 跳过。

| 测试方法 | 输入 | 验证 |
|----------|------|------|
| `test_regular_message` | `Hello!` | bot 回复非空 |
| `test_chinese_message` | `你好` | bot 回复非空 |

## 添加新测试用例

### 步骤

1. 打开 `test_bot.py`
2. 在合适的 class 里添加 `test_` 开头的方法
3. 使用 `runner.inject()` 发送消息
4. 使用 `runner.wait_for_reply()` 等待回复
5. 用 `assert` 验证结果

### 模板

```python
def test_my_feature(self, runner: BotTestRunner):
    """描述这个测试验证什么"""
    runner.inject("/mycommand", chat_id=123)
    msg = runner.wait_for_reply(count_before=0, timeout=TIMEOUT_COMMAND)

    assert msg is not None, "Bot 未回复"
    assert "expected text" in msg["text"], f"回复内容不符: {msg['text']}"
```

### 超时选择

- 命令类（`/start`, `/new` 等）：用 `TIMEOUT_COMMAND`（10s）
- 普通消息（需要 LLM）：用 `TIMEOUT_LLM`（30s）

### BotTestRunner API

| 方法 | 说明 |
|------|------|
| `runner.inject(text, chat_id, username, is_group)` | 注入用户消息 |
| `runner.inject_callback(data, chat_id, message_id)` | 注入按钮回调 |
| `runner.wait_for_reply(count_before, timeout)` | 等待新消息，返回最新一条 |
| `runner.get_sent_messages()` | 获取所有已发消息列表 |
| `runner.clear()` | 清空消息记录（每个测试前自动调用） |
| `runner.health()` | 检查 Mock Server 是否在线 |

### 多轮对话示例

```python
def test_multi_turn(self, runner: BotTestRunner):
    """测试多轮对话"""
    # 第一轮
    runner.inject("你好", chat_id=123)
    msg1 = runner.wait_for_reply(count_before=0, timeout=TIMEOUT_LLM)
    assert msg1 is not None

    # 第二轮（注意 count_before 要更新）
    count = len(runner.get_sent_messages())
    runner.inject("继续", chat_id=123)
    msg2 = runner.wait_for_reply(count_before=count, timeout=TIMEOUT_LLM)
    assert msg2 is not None
```

### 按钮回调示例

```python
def test_callback_button(self, runner: BotTestRunner):
    """测试按钮点击"""
    runner.inject_callback("s:my-topic", chat_id=123, message_id=100)
    msg = runner.wait_for_reply(count_before=0, timeout=TIMEOUT_COMMAND)
    assert msg is not None
```

## 调试技巧

```fish
# 查看 bot 完整日志
cat /tmp/octos_bot_test.log

# 手动启动 Mock Server（保持运行，方便调试）
PYTHONPATH=tests/telegram_mock \
  tests/telegram_mock/.venv/bin/python -c "
import time
from mock_tg import MockTelegramServer
server = MockTelegramServer()
server.start_background()
print('Mock server at http://127.0.0.1:5000')
while True: time.sleep(1)
"

# 手动注入消息
curl -X POST http://127.0.0.1:5000/_inject \
  -H 'Content-Type: application/json' \
  -d '{"text": "/start", "chat_id": 123, "username": "testuser"}'

# 查看 bot 回复
curl http://127.0.0.1:5000/_sent_messages

# 单独运行某个测试（需先手动启动 mock server 和 bot）
cd tests/telegram_mock
PYTHONPATH=. MOCK_BASE_URL=http://127.0.0.1:5000 \
  .venv/bin/pytest test_bot.py::TestBotCommands::test_start_command -v
```

## 已知限制

- LLM 测试受网络延迟影响，超时设置需留余量
- 媒体文件（图片/语音）Mock 返回假数据，不测试实际媒体处理
- 每次运行 bot 全新启动，不保留上次会话状态
