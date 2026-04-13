# Telegram Mock Testing Framework

## 原理

```
┌─────────────────────────────────────────────────────────────┐
│                     本地测试环境                              │
│                                                             │
│  ┌──────────────┐   GetUpdates    ┌──────────────────────┐  │
│  │  Mock Server │◄────────────────│   octos bot          │  │
│  │  (Python)    │  (长轮询)        │   (Rust)             │  │
│  │              │─────────────────►│                      │  │
│  │  模拟 Telegram│  updates[]      │  处理消息             │  │
│  │  API         │                 │  调用 LLM            │  │
│  │              │◄────────────────│                      │  │
│  │              │  sendMessage    │                      │  │
│  └──────────────┘                 └──────────────────────┘  │
│         ▲                                                   │
│         │ /_inject  /_sent_messages                         │
│  ┌──────────────┐                                           │
│  │  测试脚本     │                                           │
│  │  run_test.fish│                                          │
│  └──────────────┘                                           │
└─────────────────────────────────────────────────────────────┘
```

Mock Server 扮演两个角色：
1. **模拟 Telegram 服务器**：响应 bot 的 `GetUpdates`、`sendMessage` 等 API 调用
2. **测试控制器**：通过 `/_inject` 注入用户消息，通过 `/_sent_messages` 读取 bot 的回复

Bot 通过环境变量 `TELOXIDE_API_URL=http://127.0.0.1:5000` 指向 Mock Server，
无需真实 Telegram 网络连接。

---

## 所需资源

| 资源 | 说明 |
|------|------|
| `ANTHROPIC_API_KEY` | Anthropic API key（bot 调用 LLM 需要） |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token（格式验证用，不会真正连接 Telegram） |
| Python 3.11+ | Mock server 运行环境 |
| `uv` | Python 包管理器（`brew install uv`） |
| Rust / Cargo | 编译 octos bot |

---

## 快速开始

```fish
# 设置环境变量
set -x ANTHROPIC_API_KEY "sk-ant-..."
set -x TELEGRAM_BOT_TOKEN "123456:ABC..."

# 从项目根目录运行
fish tests/telegram_mock/run_test.fish
```

脚本会自动完成：
- 创建 Python venv 并安装依赖（首次运行）
- 写入测试用 config（`.octos/test_config.json`）
- 启动 Mock Server
- 编译并启动 octos bot（指向 Mock Server）
- 执行测试用例
- 清理所有进程

---

## 文件结构

```
tests/telegram_mock/
├── README.md           # 本文档
├── run_test.fish       # 一键测试脚本（入口）
├── mock_tg.py          # Mock Telegram API 服务器
├── test_bot.py         # pytest 测试用例集
├── runner.py           # 测试运行器工具类
├── __init__.py
└── requirements.txt    # Python 依赖
```

---

## 当前测试用例

### Test 1: `/start` 命令
- 注入 `/start` 消息
- 验证 bot 回复了命令帮助信息
- 预期：bot 返回可用命令列表
- 超时：10s（本地命令，无需 LLM）

### Test 2: 普通文本消息
- 注入 `Hello!` 消息
- 验证 bot 调用 LLM 并回复
- 预期：bot 返回任意非空回复
- 超时：15s（需要 LLM API 调用）

---

## 两类消息的区别

| 类型 | 示例 | 处理方式 | 响应时间 |
|------|------|----------|----------|
| Bot 命令 | `/start` `/new` `/sessions` | 本地处理，无需 LLM | < 1s |
| 普通消息 | `Hello!` `帮我写代码` | 调用 LLM API | 3~15s |

测试命令类消息时用较短超时（5~10s），测试 LLM 回复时用较长超时（15~30s）。

---

## 如何编写新测试用例

在 `run_test.fish` 的测试区块中添加，或在 `test_bot.py` 中用 pytest 编写。

### 方式一：在 run_test.fish 中添加（快速验证）

找到测试区块，按照现有格式添加：

```python
# ── Test 3: /new 命令 ──
print('  Test 3: /new command')
r = await client.get(f'{base}/_sent_messages')
count_before = len(r.json())
await client.post(f'{base}/_inject', json={
    'text': '/new test-session',
    'chat_id': 123,
    'username': 'testuser'
})
replied = False
for _ in range(10):
    await asyncio.sleep(1)
    r = await client.get(f'{base}/_sent_messages')
    msgs = r.json()
    if len(msgs) > count_before:
        preview = msgs[-1]['text'][:80].replace('\n', ' ')
        print(f'    \033[32m✅ Bot replied:\033[0m {preview}')
        passed += 1
        replied = True
        break
if not replied:
    print('    \033[31m❌ No reply received\033[0m')
    failed += 1
```

### 方式二：在 test_bot.py 中用 pytest 编写（推荐）

```python
@pytest.mark.asyncio
async def test_new_session_command(self, mock_server):
    mock_server.clear()
    mock_server.inject_message("/new my-session", chat_id=123)
    
    # 等待回复（命令类用短超时）
    for _ in range(10):
        await asyncio.sleep(1)
        msgs = mock_server.get_sent_messages()
        if msgs:
            assert "my-session" in msgs[-1].text or "session" in msgs[-1].text.lower()
            return
    
    pytest.fail("Bot did not reply to /new command")
```

### Mock Server 控制 API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/_inject` | POST | 注入用户消息 |
| `/_sent_messages` | GET | 获取 bot 发出的所有消息 |
| `/_clear` | POST | 清空消息记录 |
| `/health` | GET | 健康检查 |

`/_inject` 请求体：
```json
{
  "text": "消息内容",
  "chat_id": 123,
  "username": "testuser",
  "is_group": false
}
```

---

## 已知限制

1. **LLM 测试不稳定**：普通消息需要调用真实 LLM API，受网络延迟影响，超时设置需要留余量
2. **无媒体文件测试**：当前 Mock 对图片/语音返回假数据，不测试实际媒体处理
3. **单用户场景**：当前测试用例只模拟单个用户（chat_id=123），多用户并发未覆盖
4. **会话状态**：每次运行脚本 bot 是全新启动，不保留上次会话状态

---

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
```
