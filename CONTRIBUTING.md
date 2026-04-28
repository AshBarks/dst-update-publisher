# 贡献指南

感谢你对 DST Update Publisher 项目的关注！本文档提供二次开发和贡献相关的信息。

## 项目结构

```
src/
├── main.rs        # 入口，处理流程编排（单次/轮询模式）
├── lib.rs         # 模块声明
├── cli.rs         # CLI 参数解析（clap）
├── config.rs      # 配置加载（环境变量 + .env）
├── models.rs      # 数据模型（RssUpdateItem, UpdateNotification, AppConfig, ProcessOutcome 等）
├── error.rs       # 错误类型定义（thiserror）
├── rss.rs         # RSS 源抓取与解析
├── update_page.rs # 更新页面 HTML 抓取与解析（scraper）
├── po_search.rs   # PO 文件索引加载与术语搜索（含变形规则与 HashMap 索引）
├── translator.rs  # LLM 翻译逻辑（async-openai，标准 Function Calling 流程）
├── publisher.rs   # Redis 连接、去重检查、消息发布
```

## 依赖

| 库 | 用途 |
|---|---|
| `reqwest` | HTTP 请求（RSS 源与更新页面抓取） |
| `rss` | RSS XML 解析 |
| `scraper` | HTML 解析与选择器查询 |
| `zip` | ZIP 文件解压（游戏 PO 本地化包） |
| `polib` | PO 文件格式解析 |
| `redis` | Redis 客户端（Pub/Sub 与去重） |
| `tokio` | 异步运行时 |
| `serde` / `serde_json` | 序列化/反序列化 |
| `tracing` / `tracing-subscriber` | 日志追踪 |
| `dotenvy` | .env 文件加载 |
| `thiserror` | 错误处理 |
| `async-openai` | OpenAI 兼容 LLM API 客户端 |
| `chrono` | 时间处理 |
| `url` | URL 解析 |
| `clap` | CLI 参数解析 |
| `html2text` | HTML 转纯文本 |

## 构建与开发

### 环境要求

- **Rust** 1.85+（edition 2024）
- **Redis** 服务器（用于消息推送与去重）
- **LLM API**：兼容 OpenAI Chat Completion API 的服务（支持 Function Calling），如 OpenAI、Azure OpenAI 或其他兼容服务
- **游戏 PO 文件包**：DST 游戏本地化 scripts.zip（包含中文 PO 文件）

### 构建

```bash
cargo build --release
```

### 日志

通过 `RUST_LOG` 环境变量控制日志级别：

```bash
RUST_LOG=dst_update_publisher=debug cargo run --release
```

### 测试

```bash
cargo test
```

### 代码检查

```bash
cargo clippy
```

## 部署

### Docker

```dockerfile
FROM rust:1.85 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/dst-update-publisher /usr/local/bin/
COPY .env.example /app/.env.example
WORKDIR /app
ENTRYPOINT ["dst-update-publisher"]
```

构建与运行：

```bash
docker build -t dst-update-publisher .
docker run -d \
  --name dst-update-publisher \
  --env-file .env \
  -v /path/to/scripts.zip:/app/scripts.zip \
  dst-update-publisher -i 300
```

### 直接部署

1. 编译 release 版本：`cargo build --release`
2. 将二进制文件与 `.env` 文件部署到目标服务器
3. 确保 Redis 服务可达
4. 确保 PO 文件包路径正确
5. 使用 systemd 或其他进程管理工具运行：

```ini
[Unit]
Description=DST Update Publisher
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/dst-update-publisher
ExecStart=/opt/dst-update-publisher/dst-update-publisher -i 300
Restart=always
RestartSec=60

[Install]
WantedBy=multi-user.target
```

## 架构概要

### 搜索索引

PO 术语搜索使用三阶段策略：

1. **精确匹配**：通过 `exact_map`（HashMap）O(1) 查找
2. **变形匹配**：对查询词应用英文变形规则（复数、时态等）生成变体后查 HashMap
3. **模糊匹配**：通过 `word_index`（单词级反向索引）缩小候选集后做子串验证

### LLM 翻译流程

采用标准 OpenAI Function Calling 流程：

1. 发送 system prompt + user prompt（含工具定义）给 LLM
2. LLM 返回 `tool_calls`（调用 `search_po_terms` 查询术语）
3. 将 assistant message（含 tool_calls）和 tool result 追加到对话历史
4. 在同一上下文中继续请求 LLM，生成最终翻译
5. 如果 LLM 直接返回翻译（未调用工具），直接使用该结果

### 处理结果

`process_once` 返回 `ProcessOutcome` 枚举，而非用错误类型表示正常流程：

- `Published` — 成功翻译并推送
- `AlreadyProcessed` — 该 build 已处理过，跳过
- `NoUpdateAvailable` — RSS 中无 PC 更新
