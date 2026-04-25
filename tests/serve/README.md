# Octos Serve 测试说明

## 概述

本目录包含 `octos serve` 命令的功能测试，验证 REST API、SSE 流式响应、Dashboard Web UI、认证机制等核心功能。

## 测试用例列表

| 编号 | 功能 | 测试内容 | 预期结果 |
|------|------|----------|----------|
| 8.1 | Server Startup | 启动 octos serve 并监听端口 | 服务在 15 秒内启动成功，/api/status 返回 200 |
| 8.2 | REST API Sessions | GET /api/sessions | 返回 JSON 数组（Content-Type: application/json） |
| 8.3 | SSE Streaming | POST /api/chat | 收到多个 data: {...} SSE 事件流 |
| 8.4 | Dashboard WebUI | 访问 /admin/ | 返回 HTML 页面（Web UI 加载） |
| 8.5 | Auth Token Required | 无 token 请求受保护端点 | 返回 401 Unauthorized |
| 8.6 | Bind Address External | --host 0.0.0.0 | 从本地可访问（模拟外部访问） ⚠️ |
| 8.7 | Bind Address Local Default | 不加 --host 参数 | 默认绑定 127.0.0.1 ⚠️ |

## 运行测试

### 前置条件

1. **编译 octos 二进制文件**：
   ```bash
   cargo build --release
   # 或调试版本
   cargo build
   ```

2. **安装 Python 依赖**：
   ```bash
   pip install httpx pytest
   ```

### 方式一：使用 pytest 运行（推荐）

```bash
cd tests/serve
pytest test_serve.py -v

# 运行单个测试
pytest test_serve.py::test_8_1_startup -v

# 运行特定范围的测试
pytest test_serve.py -k "8_1 or 8_2" -v
```

测试完成后，报告会：
- **直接输出到 stdout**（终端显示）
- **保存到文件**：`tests/test-results/SERVE_TEST_REPORT_YYYY-MM-DD_HHMM.md`
- **日志文件**：`tests/serve/logs/serve_test_*.log`

### 方式二：直接运行脚本

```bash
cd tests/serve
python3 test_serve.py

# 指定二进制路径
python3 test_serve.py --binary ../../target/release/octos

# 详细输出
python3 test_serve.py --verbose
```

测试完成后同样会输出报告到 stdout 并保存文件。

## 测试报告

测试完成后会自动生成 Markdown 格式的报告：

### 报告输出位置

1. **stdout 输出**：测试结束后直接在终端显示完整报告
2. **报告文件**：`tests/test-results/SERVE_TEST_REPORT_YYYY-MM-DD_HHMM.md`
3. **日志文件**：`tests/serve/logs/serve_test_YYYYMMDD_HHMMSS.log`

### 报告内容

报告包含：
- 每个测试用例的执行结果（PASS/FAIL）
- ❌ 失败用例的详细错误信息专区
- ⚠️ 测试注意事项和限制说明
- 时间戳和二进制路径信息

### 查看历史报告

```bash
# 查看所有报告
ls -lt tests/test-results/SERVE_TEST_REPORT_*.md

# 查看最新报告
cat tests/test-results/SERVE_TEST_REPORT_*.md | head -100
```

## ⚠️ 重要注意事项

### 测试 8.6 和 8.7 的环境限制

#### 8.6 绑定地址测试 (--host 0.0.0.0)

**问题**：此测试在当前单机环境中无法完全验证真正的"外部可访问性"。

**原因**：
- 我们只能从 `127.0.0.1` 访问绑定到 `0.0.0.0` 的服务
- 真正的外部访问需要多网络接口环境（如局域网其他机器）

**当前实现**：
- 验证服务可以绑定到 `0.0.0.0`
- 从本地回环地址访问成功即认为通过
- 在报告中明确标注此限制

**建议**：
- 在实际部署环境中，使用另一台机器访问服务 IP 来验证
- 或使用 Docker 容器网络隔离来模拟外部访问

#### 8.7 默认只绑本地测试

**问题**：无法在单机上模拟"外部访问被拒绝"的场景。

**原因**：
- 默认绑定 `127.0.0.1` 时，从其他网络接口确实无法访问
- 但测试脚本运行在同一台机器上，无法真正验证"外部拒绝"

**当前实现**：
- 验证默认绑定地址为 `127.0.0.1`
- 确认可以从 `127.0.0.1` 访问
- 在日志中注明需要多接口环境才能真正测试

**建议**：
- 在生产环境中，检查 `netstat -tuln | grep 8080` 确认绑定地址
- 或使用防火墙规则验证外部访问被阻止

### 其他注意事项

1. **端口占用**：
   - 测试使用端口 8080 和 8081
   - 确保这些端口未被其他服务占用
   - 测试结束后会自动清理

2. **临时文件清理**：
   - 每个测试会话创建独立的临时数据目录
   - 测试结束后自动删除（`tempfile.TemporaryDirectory`）
   - 如果测试异常中断，可能需要手动清理 `/tmp` 下的临时目录

3. **超时设置**：
   - 服务启动超时：15 秒
   - HTTP 请求超时：5-10 秒
   - SSE 流式测试：10 秒

4. **认证令牌**：
   - 测试使用固定 token: `test-token-12345`
   - 可通过修改 `OctosServeTester.auth_token` 自定义

5. **日志文件**：
   - 测试日志保存在 `tests/test-results/serve_logs/`
   - 包含服务器启动日志和测试执行日志
   - 便于调试失败的测试用例

## 故障排查

### 测试启动失败

**症状**：`Failed to start octos serve for testing`

**可能原因**：
1. 二进制文件不存在或不可执行
2. 端口 8080 已被占用
3. 权限不足

**解决方法**：
```bash
# 检查二进制文件
ls -l target/debug/octos

# 检查端口占用
lsof -i :8080  # macOS/Linux
netstat -ano | findstr :8080  # Windows

# 查看测试日志
tail -f tests/test-results/serve_logs/serve_test_*.log
```

### SSE 测试超时

**症状**：`test_8_3_sse_streaming` 超时或失败

**可能原因**：
1. LLM 提供商未配置，无法生成响应
2. 网络连接问题

**解决方法**：
- 确保 `config.json` 中配置了有效的 LLM 提供商
- 或设置环境变量 `OPENAI_API_KEY` 等
- 查看服务器日志确认是否有错误

### Dashboard 测试失败

**症状**：`test_8_4_dashboard` 返回非 200 状态码

**可能原因**：
1. 静态文件未正确嵌入二进制
2. 路由配置问题

**解决方法**：
```bash
# 检查静态文件是否存在
ls -la crates/octos-cli/static/

# 重新编译
cargo clean && cargo build
```

## 扩展测试

如需添加新的测试用例：

1. 在 `OctosServeTester` 类中添加测试方法：
   ```python
   def test_8_X_new_feature(self) -> bool:
       """测试描述"""
       # 测试逻辑
       return True
   ```

2. 添加 pytest 测试函数：
   ```python
   def test_8_X_new_feature(serve_tester):
       result = serve_tester.run_test("8.X", "新功能", serve_tester.test_8_X_new_feature)
       assert result.status == "PASS", result.details
   ```

3. 更新本文档的测试用例列表

## 技术实现细节

### 进程管理

- 使用 `subprocess.Popen` 启动服务器
- 捕获 stdout/stderr 用于日志记录
- 优雅关闭：先 `terminate()`，超时后 `kill()`

### HTTP 客户端

- 使用 `httpx` 库（支持 async/await）
- SSE 测试使用 `httpx.stream()` 逐行读取事件
- 自动处理连接重试和超时

### 测试隔离

- 每个测试会话使用独立的临时数据目录
- 避免测试之间的状态污染
- 自动清理临时文件和进程

### 认证机制

- 支持 Bearer Token 认证
- 测试使用固定 token 简化流程
- 验证无 token 请求返回 401

## 参考文档

- [Octos Serve 命令实现](../../crates/octos-cli/src/commands/serve.rs)
- [API 路由定义](../../crates/octos-cli/src/api/router.rs)
- [SSE 广播实现](../../crates/octos-cli/src/api/sse.rs)
- [静态文件服务](../../crates/octos-cli/src/api/static_files.rs)
