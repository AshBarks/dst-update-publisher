# DST Update Publisher

《饥荒联机版》（Don't Starve Together）更新公告自动翻译与推送工具。

基于 Klei 官方论坛的 RSS 更新源，抓取更新公告，利用 LLM（大语言模型）将英文公告翻译为中文，并通过游戏 PO 本地化文件确保术语翻译准确，最终将翻译结果以 JSON 格式发布到 Redis Pub/Sub 通道。

## 功能

- **RSS 监控**：从 Klei 官方论坛 RSS 源获取 DST PC 版更新公告，自动筛选 PC 平台条目
- **版本信息抓取**：解析官方更新页面 HTML，获取 build number、revision、发布通道（release/beta）及是否为 hotfix
- **PO 术语查询**：从游戏本地化 ZIP 包中加载 PO 文件，建立术语索引，供 LLM 翻译时查询专有术语的官方译名
- **LLM 翻译**：通过 OpenAI 兼容 API 调用大语言模型，使用 Function Calling（`search_po_terms` 工具）实现术语辅助翻译
- **Redis 推送**：将翻译后的更新通知以 JSON 格式通过 Redis `PUBLISH` 命令推送到指定通道
- **去重机制**：使用 Redis 键记录已处理的 build number，避免重复翻译和推送同一版本
- **双运行模式**：支持单次执行（`once`）和持续轮询（`poll`）两种运行方式

## 项目结构

```
src/
├── main.rs        # 入口，处理流程编排（单次/轮询模式）
├── lib.rs         # 模块声明
├── cli.rs         # CLI 参数解析（clap）
├── config.rs      # 配置加载（环境变量 + .env）
├── models.rs      # 数据模型（RssUpdateItem, UpdateNotification, AppConfig 等）
├── error.rs       # 错误类型定义（thiserror）
├── rss.rs         # RSS 源抓取与解析
├── update_page.rs # 更新页面 HTML 抓取与解析（scraper）
├── po_search.rs   # PO 文件索引加载与术语搜索
├── translator.rs  # LLM 翻译逻辑（async-openai，含 tool calling）
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
| `thiserror` / `anyhow` | 错误处理 |
| `async-openai` | OpenAI 兼容 LLM API 客户端 |
| `chrono` | 时间处理 |
| `url` | URL 解析 |
| `clap` | CLI 参数解析 |
| `html2text` | HTML 转纯文本 |
| `once_cell` | 单次初始化 |

## 环境要求

- **Rust** 1.85+（edition 2024）
- **Redis** 服务器（用于消息推送与去重）
- **LLM API**：兼容 OpenAI Chat Completion API 的服务（支持 Function Calling），如 OpenAI、Azure OpenAI 或其他兼容服务
- **游戏 PO 文件包**：DST 游戏本地化 scripts.zip（包含中文 PO 文件）

## 配置

复制 `.env.example` 为 `.env`，根据实际情况填写：

```bash
cp .env.example .env
```

### 配置项说明

| 变量 | 必填 | 说明 | 默认值 |
|---|---|---|---|
| `RSS_URL` | 否 | DST 更新 RSS 源地址 | Klei 官方论坛 RSS |
| `UPDATE_PAGE_URL` | 否 | DST 更新页面地址 | Klei 官方更新页面 |
| `REDIS_URL` | 否 | Redis 连接地址 | `redis://127.0.0.1:6379` |
| `REDIS_CHANNEL` | 否 | Redis Pub/Sub 发布通道 | `dst-updates` |
| `REDIS_DEDUPE_KEY` | 否 | 去重用的 Redis 键名 | `dst:last_build` |
| `LLM_API_BASE` | 是 | LLM API 基础 URL | - |
| `LLM_API_KEY` | 是 | LLM API 密钥 | - |
| `LLM_MODEL` | 否 | 使用的模型名称 | `gpt-4o` |
| `PO_ZIP_PATH` | 是 | 游戏本地化 ZIP 文件路径 | - |
| `PO_ZIP_PO_FILES` | 是 | ZIP 中 PO 文件路径（逗号分隔） | - |

### PO 文件说明

`PO_ZIP_PATH` 指向 DST 游戏的 `scripts.zip` 本地化文件包。`PO_ZIP_PO_FILES` 指定 ZIP 包内需要加载的 PO 文件路径，例如：

```
PO_ZIP_PO_FILES=scripts/languages/chinese_s.po,scripts/languages/chinese_t.po
```

同时加载简体中文和繁体中文 PO 文件可提供更完整的术语覆盖。

## 使用

### 构建

```bash
cargo build --release
```

### 运行模式

#### 单次执行

执行一次完整的抓取→翻译→推送流程后退出：

```bash
cargo run --release
# 或直接运行编译产物
./dst-update-publisher
```

#### 持续轮询

以指定间隔（秒）持续监控 RSS 源，发现新更新时自动处理：

```bash
cargo run --release -- --poll-interval 300
# 或
./dst-update-publisher -i 300
```

`-i` / `--poll-interval` 参数指定轮询间隔秒数，设置后进入轮询模式；不设置则为单次执行模式。

### 日志

通过 `RUST_LOG` 环境变量控制日志级别：

```bash
RUST_LOG=dst_update_publisher=debug cargo run --release
```

## 输出格式

发布到 Redis 通道的 JSON 消息格式：

```json
{
  "build_number": "...",
  "revision": "...",
  "channel": "release | beta",
  "is_hotfix": true | false,
  "original_description": "英文原文",
  "translated_description": "中文译文",
  "glossary": {
    "术语英文": "术语中文"
  },
  "pub_date": "RFC3339 时间",
  "link": "更新公告链接"
}
```

## 工作流程

```
1. 从 RSS 源获取最新 PC 更新条目
          ↓
2. 检查 Redis 去重键，判断该 build 是否已处理
          ↓ (未处理)
3. 抓取更新页面 HTML，获取版本详细信息
          ↓
4. 将 RSS 公告 HTML 转为纯文本
          ↓
5. 调用 LLM 翻译：
   a. 第一轮：LLM 通过 search_po_terms 工具提取并查询术语
   b. 第二轮：将术语查询结果与原文一起发送给 LLM，生成最终译文
          ↓
6. 组合 UpdateNotification（原文 + 译文 + 术语表 + 版本信息）
          ↓
7. PUBLISH 到 Redis 通道
          ↓
8. 在 Redis 中标记该 build 已处理
```

## 部署

### Docker（推荐）

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
# systemd 示例
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

### 下游消费

下游服务通过 Redis `SUBSCRIBE` 命令订阅 `dst-updates` 通道即可接收更新通知，例如：

- 机器人推送（Discord / QQ / 微信等）
- 网站内容更新
- 通知邮件发送