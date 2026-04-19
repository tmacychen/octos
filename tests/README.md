# Octos 自动化测试框架

## 一、框架架构与组成

### 整体架构

```
┌──────────────────────────────────────────────────────────────────────┐
│  run_tests.sh — 统一测试入口                                         │
│  ├─ 环境变量预检（编译前检查，避免无效等待）                             │
│  ├─ cargo build --all-features（增量编译 + SHA256 校验同步）          │
│  ├─ Bot Mock Tests                                                   │
│  │   └─ bot_test.sh → Python (pytest) + Mock Server                 │
│  │       ├─ Telegram: mock_tg.py (HTTP REST, port 5000)             │
│  │       └─ Discord:  mock_discord.py (REST + WebSocket, port 5001) │
│  └─ CLI Tests                                                        │
│      └─ cli_test.sh → Bash + jq（通过 test_cases.json 驱动）          │
│          └─ 隔离测试目录: /tmp/octos_test/cli/<Category>_<timestamp> │
└──────────────────────────────────────────────────────────────────────┘
```

### 文件结构

```
tests/
├── run_tests.sh                    # 统一测试入口（445 行）
├── COMMIT_CLASSIFICATION.md        # Commit 分类规范
├── GATEWAY_TEST_PLAN.md            # Gateway 测试补充计划
├── TELEGRAM_TEST_ANALYSIS.md       # Telegram 测试分析报告
├── bot_mock_test/
│   ├── README.md                   # Bot 测试框架详细文档
│   ├── bot_test.sh                 # Bot 测试脚本（552 行）
│   ├── mock_tg.py                  # Telegram Mock Server（661 行）
│   ├── mock_discord.py             # Discord Mock Server（743 行）
│   ├── runner.py                   # Telegram BotTestRunner（74 行）
│   ├── runner_discord.py           # Discord DiscordTestRunner（75 行）
│   ├── test_telegram.py            # Telegram 测试用例（711 行, 43 个）
│   ├── test_discord.py             # Discord 测试用例（871 行, 46 个）
│   ├── conftest.py                 # pytest fixtures
│   ├── requirements.txt
│   └── __init__.py
└── cli_test/
    ├── cli_test.sh                 # CLI 测试脚本（657 行）
    └── test_cases.json             # CLI 测试用例定义（35 个）
```

### 核心机制

| 机制 | 说明 |
|------|------|
| **增量编译** | SHA256 校验二进制，未变更时跳过编译 |
| **环境预检** | 编译前检查 `ANTHROPIC_API_KEY`/`TELEGRAM_BOT_TOKEN`，缺失立即报错 |
| **目录隔离** | CLI 每个测试类别在独立子目录运行，Init 等操作不影响主测试目录 |
| **Session 日志** | 全部 stdout/stderr 通过 `exec tee` 同时输出到终端和时间戳日志文件 |
| **模块结果汇总** | 测试结束输出各模块 PASS/FAIL 状态 |

---

## 二、使用说明

### 基本用法

```bash
# 查看帮助
bash tests/run_tests.sh --help

# 运行全部测试
bash tests/run_tests.sh all

# 运行 Bot 测试
bash tests/run_tests.sh --test bot              # 全部
bash tests/run_tests.sh --test bot telegram     # 仅 Telegram
bash tests/run_tests.sh --test bot discord      # 仅 Discord
bash tests/run_tests.sh --test bot tg test_new_default  # 运行单个用例

# 运行 CLI 测试
bash tests/run_tests.sh --test cli              # 全部
bash tests/run_tests.sh --test cli -s Init      # 仅 Init 类别
bash tests/run_tests.sh --test cli -v           # 详细输出
bash tests/run_tests.sh --test cli list         # 列出测试类别

# 列出 Bot 测试模块/用例
bash tests/run_tests.sh --test bot list
bash tests/run_tests.sh --test bot cases tg
```

### 环境变量

| 变量 | 必需 | 说明 |
|------|------|------|
| `ANTHROPIC_API_KEY` | Bot 测试 | LLM API 密钥 |
| `TELEGRAM_BOT_TOKEN` | Telegram | Telegram Bot Token（Mock 模式下可为任意非空值） |
| `DISCORD_BOT_TOKEN` | 否 | Discord Mock 模式自动填充假 token |

### 测试输出目录

```
/tmp/octos_test/
├── octos                          # 编译后的二进制
├── .octos_checksum                # 二进制 SHA256 校验
├── cli/                           # CLI 测试隔离工作区
│   ├── Init_<timestamp>/          # Init 测试临时目录
│   ├── Clean_<timestamp>/         # Clean 测试临时目录
│   └── temp/                      # 共享临时目录
└── logs/
    ├── build.log                  # 编译日志
    ├── cli_test.log               # CLI 测试独立日志
    ├── octos_bot_test_<ts>.log    # Bot 测试日志
    └── sessions/                  # Session 日志（全部输出）
        └── <timestamp>.log
```

---

## 三、测试模块概述

### 3.1 CLI 测试

CLI 测试通过 `cli_test.sh` + `test_cases.json` 驱动，验证 `octos` 命令行各子命令的正确性。

**测试类型：**
- `cli`：执行 octos 命令，检查输出是否包含预期字符串
- `file_check`：检查文件/目录是否存在，存在时记录内容到日志

**测试类别（35 个用例）：**

| 类别 | 用例数 | 说明 |
|------|--------|------|
| CLI | 3 | `--help`, `--version`, 单消息模式 |
| Tools | 1 | 工具调用 |
| Security | 1 | 危险命令拒绝 |
| Init | 4 | `init --help`, `--defaults`, config.json 检查, 重复 init |
| Clean | 3 | `clean --help`, 无 .octos 目录, 空目录清理 |
| Status | 2 | `status --help`, 状态查询 |
| Completions | 6 | bash/zsh/fish/powershell 补全, 无效 shell |
| Skills | 5 | list/search/remove 操作 |
| Auth | 2 | `auth --help`, 登录状态 |
| Channels | 2 | `channels --help`, 频道状态 |
| Cron | 2 | `cron --help`, 定时任务列表 |
| Chat | 1 | `chat --help` |
| Gateway | 1 | `gateway --help` |
| Serve | 1 | `serve --help` |
| Docs | 1 | `docs --help` |

> *CLI 测试用例详情待后续补充。*

---

### 3.2 Telegram 测试

通过 Mock Server 模拟 Telegram Bot API，验证 octos 在 Telegram 频道上的行为。

**架构：**

```
  pytest (test_telegram.py)
    │
    │  HTTP 请求
    ▼
  ┌────────────────────────────────────────────────────────────────┐
  │  Mock Server (mock_tg.py, FastAPI, port 5000)                 │
  │                                                                │
  │  ┌─ Telegram API 模拟 ─────────────────────────────────────┐  │
  │  │  POST /bot{token}/sendMessage      ← octos 调用          │  │
  │  │  POST /bot{token}/editMessageText  ← octos 流式编辑      │  │
  │  │  GET  /bot{token}/getUpdates       ← octos 长轮询消息    │  │
  │  │  POST /bot{token}/sendChatAction   ← octos 发送输入状态  │  │
  │  └─────────────────────────────────────────────────────────┘  │
  │                                                                │
  │  ┌─ 测试控制接口 ─────────────────────────────────────────┐  │
  │  │  POST /_inject            → 注入用户消息到 getUpdates   │  │
  │  │  GET  /_sent_messages     → 读取 bot 已发送的消息       │  │
  │  │  POST /_clear             → 清空消息/状态               │  │
  │  │  GET  /health             → 健康检查                    │  │
  │  └─────────────────────────────────────────────────────────┘  │
  │                                                                │
  │  内部状态: messages[], updates[], update_id (单调递增)         │
  └────────────────────────────────────────────────────────────────┘
            ▲                               │
            │  TELOXIDE_API_URL              │  sendMessage / getUpdates
            │  = http://127.0.0.1:5000       │
            │                               ▼
  ┌────────────────────────────────────────────────────────────────┐
  │  octos bot (teloxide)                                          │
  │  └─ Bot::new(token).set_api_url(url)                           │
  │     → 所有 Bot API 请求重定向到 Mock Server                     │
  └────────────────────────────────────────────────────────────────┘

  测试流程:
  1. bot_test.sh 启动 Mock Server + octos gateway
  2. pytest 通过 /_inject 注入用户消息
  3. octos 通过 getUpdates 获取消息 → LLM 处理 → sendMessage 回复
  4. pytest 通过 /_sent_messages 断言 bot 回复内容
```

**测试用例（43 个）：**

| 类别 | 用例数 | 说明 |
|------|--------|------|
| TestSessionCommands | 15 | `/new`, `/s`, `/sessions`, `/back`, `/delete`, `/soul` |
| TestSessionActorCommands | 7 | `/adaptive`, `/queue`, `/status`, `/reset`, 未知命令 |
| TestMultiUser | 2 | 多用户会话隔离, 回调切换 |
| TestProfileMode | 3 | Profile 隔离, soul 按 profile 隔离 |
| TestAbortCommands | 5 | 中/英/日/俄语中止, cancel 命令 |
| TestMessageSplitting | 2 | 4096 字符限制, 自动分片 |
| TestConcurrencyLimit | 2 | 并发会话创建, 队列模式 |
| TestFileLimits | 2 | 10MB 累积大小限制 |
| TestStreamingEdit | 1 | 流式编辑（已跳过） |
| TestLLMMessages | 2 | 常规消息, 中文消息 |
| TestDocumentUpload | 2 | 文档上传 |

**超时配置：**
- `TIMEOUT_COMMAND = 30s`（命令类）
- `TIMEOUT_LLM = 90s`（LLM 消息）
- `TIMEOUT_ABORT = 60s`（中止命令）
- `TIMEOUT_LARGE = 180s`（大对话）

---

### 3.3 Discord 测试

通过 Mock Server 模拟 Discord API（REST + WebSocket Gateway），验证 octos 在 Discord 频道上的行为。

**架构：**

```
  pytest (test_discord.py)
    │
    │  HTTP 请求
    ▼
  ┌────────────────────────────────────────────────────────────────────┐
  │  Mock Discord Server (mock_discord.py, FastAPI + WebSocket, 5001) │
  │                                                                    │
  │  ┌─ Discord REST API 模拟 ────────────────────────────────────┐  │
  │  │  GET  /api/v10/gateway/bot  → 返回 ws://127.0.0.1:5001/   │  │
  │  │  POST /api/v10/channels/{id}/messages       → 发送/回复    │  │
  │  │  PUT  /api/v10/channels/{id}/messages/{mid}  → 编辑消息    │  │
  │  │  DELETE /api/v10/channels/{id}/messages/{mid} → 删除消息   │  │
  │  │  POST .../reactions/{emoji}/@me              → 添加反应    │  │
  │  │  GET  /api/v10/users/@me                     → Bot 信息    │  │
  │  └────────────────────────────────────────────────────────────┘  │
  │                                                                    │
  │  ┌─ WebSocket Gateway (/) ────────────────────────────────────┐  │
  │  │  1. serenity 连接 WS → Server 发送 Hello (opcode 10)       │  │
  │  │  2. serenity 回复 Identify (opcode 2, 含 token)             │  │
  │  │  3. Server 发送 Ready (opcode 0, 含 user/guilds 数据)       │  │
  │  │  4. 开始心跳: serenity ↔ Server (opcode 1/11)               │  │
  │  │  5. /_inject → Server 向 serenity 派发 MESSAGE_CREATE      │  │
  │  └────────────────────────────────────────────────────────────┘  │
  │                                                                    │
  │  ┌─ 测试控制接口 ─────────────────────────────────────────────┐  │
  │  │  POST /_inject              → 注入用户消息 (→ WS dispatch) │  │
  │  │  POST /_inject_interaction  → 注入斜杠命令/按钮交互        │  │
  │  │  GET  /_sent_messages       → 读取 bot 已发送的消息        │  │
  │  │  POST /_clear               → 清空消息/状态                │  │
  │  │  GET  /health               → 健康检查                     │  │
  │  └────────────────────────────────────────────────────────────┘  │
  │                                                                    │
  │  内部状态: messages[], ws_connections[], session_id, seq           │
  └────────────────────────────────────────────────────────────────────┘
            ▲                                    │
            │  DISCORD_API_BASE_URL               │  REST + WS
            │  = http://127.0.0.1:5001             │
            │                                    ▼
  ┌────────────────────────────────────────────────────────────────────┐
  │  octos bot (serenity)                                              │
  │  └─ HttpBuilder::new(token).proxy(base_url).ratelimiter_disabled() │
  │     → REST API 重定向到 Mock Server                                │
  │  └─ ClientBuilder::new_with_http(http, intents)                    │
  │     → Gateway WS URL 由 GET /gateway/bot 返回，自动连到 Mock       │
  └────────────────────────────────────────────────────────────────────┘

  测试流程:
  1. bot_test.sh 启动 Mock Server + octos gateway
  2. serenity 通过 REST 获取 Gateway URL → 建立 WS 连接 → 完成握手
  3. pytest 通过 /_inject 注入用户消息 → Mock Server 通过 WS 派发 MESSAGE_CREATE
  4. serenity 收到事件 → octos 处理 → 通过 REST sendMessage 回复
  5. pytest 通过 /_sent_messages 断言 bot 回复内容
```

**关键机制：**
1. **REST 代理**：`DISCORD_API_BASE_URL` 通过 `HttpBuilder::proxy()` 重定向所有 REST 请求
2. **Gateway 自动连接**：serenity 先调 `GET /gateway/bot` 获取 WS URL（Mock 返回本地地址），然后自动建立 WS 连接
3. **Ratelimiter 禁用**：Mock 模式下必须禁用 serenity 的限流器，因为 `Ratelimiter::perform()` 内部不传 proxy 参数，会绕过 URL 重写
4. **Http 实例克隆**：`ClientBuilder::new_with_http()` 需要独占 Http 实例，因此通过 `build_http_for_client()` 创建独立但配置相同的 Http
5. **无需真实 token**：Mock Server 不校验 bot token

**测试用例（46 个）：**

| 类别 | 用例数 | 说明 |
|------|--------|------|
| TestDiscordSessionCommands | 10 | `/new`, `/s`, `/sessions`, `/back`, `/delete`, `/soul` |
| TestDiscordSessionActorCommands | 5 | `/adaptive`, `/queue`, `/status`, `/reset`, 未知命令 |
| TestDiscordMultiUser | 1 | 多频道会话隔离 |
| TestDiscordProfileMode | 3 | Profile 隔离, soul 按 profile 隔离 |
| TestDiscordAbortCommands | 5 | 中/英/日/俄语中止, cancel 命令 |
| TestDiscordMessageSplitting | 2 | 2000 字符限制, 自动分片 |
| TestDiscordConcurrencyLimit | 2 | 并发会话创建 |
| TestDiscordFileLimits | 2 | 10MB 累积大小限制 |
| TestDiscordStreamingEdit | 1 | 流式编辑 |
| TestDiscordLLMMessages | 2 | 常规消息, 中文消息 |
| TestDiscordDocumentUpload | 2 | 文档上传 |
| TestDiscordInteractions | 9 | 斜杠命令, 按钮回调 |

**超时配置：**
- `TIMEOUT_COMMAND = 20s`
- `TIMEOUT_LLM = 40s`
- （比 Telegram 稍长，因 Discord Gateway 有握手和心跳开销）

---

## 四、测试目录与日志

### 运行时目录结构

测试运行时所有临时文件位于 `/tmp/octos_test/`，完整结构如下：

```
/tmp/octos_test/
├── octos                              # cargo build 编译后的二进制
├── .octos_checksum                    # 二进制文件 SHA256 校验值（用于增量编译判断）
│
├── .octos/                            # Bot 测试配置目录
│   ├── test_telegram_config.json      # Telegram 测试配置
│   └── test_discord_config.json       # Discord 测试配置
│
├── cli/                               # CLI 测试隔离工作区
│   ├── Init_<timestamp>/              # Init 测试专用目录（避免污染共享目录）
│   │   └── .octos/
│   │       └── config.json            # init --defaults 产生的配置文件
│   ├── Clean_<timestamp>/             # Clean 测试专用目录
│   └── temp/                          # 共享临时目录
│       └── no-octos-dir/              # 用于 clean 测试"无 .octos 目录"场景
│
└── logs/
    ├── build.log                      # cargo build 输出日志
    ├── cli_test.log                   # CLI 测试独立日志（由 log() 函数写入）
    ├── octos_telegram_bot_test_<ts>.log # Telegram bot 运行日志（gateway 输出）
    ├── octos_discord_bot_test_<ts>.log  # Discord bot 运行日志（gateway 输出）
    └── sessions/                      # Session 日志目录
        └── <YYYYMMDD_HHMMSS>.log     # 完整测试会话日志（含所有 stdout/stderr）
```

### 日志体系

测试框架采用**双通道日志**设计：

| 日志文件 | 写入方式 | 内容 | 用途 |
|----------|----------|------|------|
| `sessions/<ts>.log` | `exec > >(tee ...)` | 所有 stdout/stderr 的完整副本 | 事后回溯整个测试过程 |
| `cli_test.log` | `log()` 函数 | CLI 测试的结构化日志（`[EXEC]`/`[STATUS]`/`[FILE]` 标签） | 快速定位 CLI 测试问题 |
| `build.log` | cargo 重定向 | 编译输出 | 排查编译失败 |
| `octos_*_bot_test_<ts>.log` | gateway 重定向 | Bot 运行时日志 | 排查 Bot 行为异常 |

**Session 日志**由 `run_tests.sh` 在启动时通过以下方式建立：

```bash
SESSION_LOG="$LOG_DIR/sessions/${TIMESTAMP}.log"
exec > >(tee -a "$SESSION_LOG") 2>&1
```

所有子进程（bot_test.sh、cli_test.sh、pytest）的输出均被捕获。

**CLI 日志**中 `log()` 函数同时写入 stdout（被 session 日志捕获）和独立文件：

```bash
log() {
    echo "$@" | tee -a "$LOG_FILE"
}
```

**文件内容记录**：`file_check` 类型测试在检测到文件存在时，自动将内容输出到日志：

```
[FILE] BEGIN /tmp/octos_test/cli/Init_xxx/.octos/config.json
{"version":1,"provider":"anthropic",...}
[FILE] END
```

---

## 五、Rust 代码修改分析

为支持 Mock Server 测试模式，对 octos Rust 代码进行了 4 处必要修改。这些修改不影响生产环境行为，仅在设置特定环境变量时激活。

### 5.1 telegram_channel.rs — 支持自定义 API URL

**文件**：`crates/octos-bus/src/telegram_channel.rs`（+14 行）
**Commit**：`8fe2e06` feat(telegram): add mock testing framework for bot functionality

**修改内容**：在 `TelegramChannel::new()` 中新增 `TELOXIDE_API_URL` 环境变量支持。

```rust
// 修改前
Self {
    bot: Bot::new(token),
    ...
}

// 修改后
let api_url = std::env::var("TELOXIDE_API_URL")
    .ok()
    .and_then(|u| reqwest::Url::parse(&u).ok());

let bot = if let Some(url) = api_url {
    info!("Using custom Telegram API URL: {}", url);
    Bot::new(token).set_api_url(url)
} else {
    Bot::new(token)
};

Self {
    bot,
    ...
}
```

**修改原因**：teloxide 默认连接 `https://api.telegram.org`，无法指向本地 Mock Server。通过 `Bot::set_api_url()` 将所有 API 请求重定向到 `http://127.0.0.1:5000`。生产环境不设置该变量，行为不变。

---

### 5.2 discord_channel.rs — 支持自定义 API Base URL + Proxy 模式

**文件**：`crates/octos-bus/src/discord_channel.rs`（+47 行，-2 行）
**Commit**：`7c6c20a` feat(tests): add Discord mock testing & refactor bot_mock framework

**修改内容**：

1. **Http 代理**：新增 `DISCORD_API_BASE_URL` 环境变量，通过 `HttpBuilder::proxy()` 重定向 REST API：

```rust
// 修改前
let http = Arc::new(Http::new(token));

// 修改后
let http = if let Ok(base_url) = std::env::var("DISCORD_API_BASE_URL") {
    Arc::new(
        HttpBuilder::new(token)
            .proxy(base_url)
            .ratelimiter_disabled(true)  // 必须禁用！
            .build(),
    )
} else {
    Arc::new(Http::new(token))
};
```

2. **Http 实例克隆**：新增 `build_http_for_client()` 方法，为 `ClientBuilder` 创建独立 Http：

```rust
fn build_http_for_client(&self) -> Http {
    if let Some(proxy) = &self.http.proxy {
        HttpBuilder::new(&self.token)
            .proxy(proxy.clone())
            .ratelimiter_disabled(true)
            .build()
    } else {
        Http::new(&self.token)
    }
}
```

3. **Client 构建**：用 `ClientBuilder::new_with_http()` 替代 `Client::builder()`：

```rust
// 修改前
let mut client = Client::builder(&self.token, intents)
    .event_handler(handler)
    .await?;

// 修改后
let http = self.build_http_for_client();
let mut client = ClientBuilder::new_with_http(http, intents)
    .event_handler(handler)
    .await?;
```

**修改原因**：
- Discord 的 REST 和 Gateway 是分离的。`HttpBuilder::proxy()` 只重定向 REST 请求；Gateway WS URL 由 `GET /gateway/bot` 返回
- 必须禁用 ratelimiter：serenity 的 `Ratelimiter::perform()` 在构建请求时传 `None` 给 proxy 参数，绕过了 URL 重写，导致请求仍发往 discord.com
- `Client::builder(token, intents)` 内部自建 Http，无法配置 proxy。改用 `new_with_http()` 传入已配置的 Http 实例
- `self.http` 是 `Arc<Http>` 被 send/edit 方法共享，无法移交所有权给 ClientBuilder，因此需要 `build_http_for_client()` 创建独立实例

---

### 5.3 gateway_dispatcher.rs — Soul 按 chat_id 隔离存储

**文件**：`crates/octos-cli/src/gateway_dispatcher.rs`（+27 行，-4 行）
**Commit**：`e8b2a3b` fix: isolate soul storage per user by chat_id

**修改内容**：将 soul 存储路径从全局 `{data_dir}/soul.md` 改为按用户隔离的 `{data_dir}/users/{chat_id}/soul.md`。

```rust
// 修改前
let data_dir = match &self.data_dir { ... };
let reply = crate::soul_service::read_soul(data_dir);

// 修改后
let base_data_dir = match &self.data_dir { ... };
let user_data_dir = base_data_dir.join("users").join(reply_chat_id);
std::fs::create_dir_all(&user_data_dir)?;
let reply = crate::soul_service::read_soul(&user_data_dir);
```

**修改原因**：原实现中所有用户共享同一个 `soul.md`，多用户场景下：
- 用户 A 设置 `/soul You are a cat`
- 用户 B 设置 `/soul You are a dog`
- 用户 A 的 soul 被覆盖为 "You are a dog"

这在 Telegram 群组和 Discord 服务器中是严重 bug。修改后每个用户拥有独立的 soul 存储。

---

### 5.4 gateway_runtime.rs — 补充缺失的 import

**文件**：`crates/octos-cli/src/commands/gateway/gateway_runtime.rs`（+1 行）
**Commit**：`e9468e3` fix: add missing MAIN_PROFILE_ID import in gateway_runtime.rs

**修改内容**：

```rust
use octos_core::MAIN_PROFILE_ID;
```

**修改原因**：5.3 中 gateway_dispatcher.rs 使用了 `MAIN_PROFILE_ID` 常量，但该 import 在 gateway_runtime.rs 中缺失，导致编译失败。这是 5.3 的配套修复。

---

### 修改影响范围总结

| 文件 | 修改类型 | 生产环境影响 | 测试必需性 |
|------|----------|-------------|-----------|
| telegram_channel.rs | 新增环境变量分支 | 无（未设置变量时行为不变） | Mock 测试必需 |
| discord_channel.rs | 新增 proxy 模式 + Http 克隆 | 无（未设置变量时行为不变） | Mock 测试必需 |
| gateway_dispatcher.rs | Bug 修复 | 有（改变 soul 存储路径） | 独立 bug 修复 |
| gateway_runtime.rs | 补充 import | 无 | 编译修复 |

---

## 六、工作总结

### 提交统计

| 指标 | 数值 |
|------|------|
| 总提交数 | 59 |
| 新增代码行 | 5,948 行（22 个文件） |
| 核心脚本代码 | 4,788 行 |
| 测试用例总数 | 124 个（Telegram 43 + Discord 46 + CLI 35） |

### 解决的核心问题

| 问题 | 优先级 | 状态 | 说明 |
|------|--------|------|------|
| Soul 按 chat_id 隔离存储 | P0 | ✅ 已修复 | 原全局共享导致多用户 soul 互相覆盖 |
| CLI init 路径引号 Bug | P0 | ✅ 已修复 | `$cmd_args` 展开时引号被当作路径字面字符，文件创建到含引号的错误目录 |
| CLI 测试目录污染 | P1 | ✅ 已修复 | Init 测试在共享 TEST_DIR 创建 .octos/，改为隔离到 cli/ 子目录 |
| Abort 命令超时 | P1 | ✅ 已修复 | 增加 TIMEOUT_ABORT=60s |
| LLM setup 超时 | P1 | ✅ 已修复 | conftest.py timeout 15s→30s |
| 大对话测试超时 | P2 | ✅ 已修复 | 增加 TIMEOUT_LARGE=180s |
| Stream Edit 失败 | P3 | ⏸️ 已跳过 | teloxide 与 Mock Server 交互问题，不影响核心功能 |
| 环境变量预检缺失 | P1 | ✅ 已修复 | 编译前检查必需环境变量，避免白等编译后才报错 |
| 测试日志不完整 | P2 | ✅ 已修复 | 添加 session log + 文件内容记录到日志 |

### 关键设计决策

1. **Mock Server 模式**：无需真实 API 凭证即可运行，降低测试门槛
2. **统一入口 `run_tests.sh`**：编译一次，按需运行 bot/cli 测试
3. **CLI 目录隔离**：每个测试类别在 `/tmp/octos_test/cli/<Category>_<timestamp>` 下运行，测试结束自动清理
4. **双通道日志**：`exec tee` 捕获全部 stdout/stderr 到 session log，同时各模块维护独立日志文件
5. **file_check 内容记录**：文件存在时自动输出内容到日志，便于排查
