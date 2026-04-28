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
  "build_number": "615492",
  "revision": "r378",
  "channel": "release",
  "is_hotfix": false,
  "original_description": "英文原文...",
  "translated_description": "中文译文...",
  "glossary": {
    "Varg": "座狼",
    "Hound": "猎犬"
  },
  "pub_date": "2025-04-28T12:00:00+00:00",
  "link": "https://forums.kleientertainment.com/game-updates/dst/update-615492-r378/"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `build_number` | string | 版本构建号 |
| `revision` | string | 版本修订号 |
| `channel` | string | 发布通道：`"release"` 或 `"beta"` |
| `is_hotfix` | boolean | 是否为热修复 |
| `original_description` | string | 英文原文（Markdown 格式） |
| `translated_description` | string | 中文译文（Markdown 格式） |
| `glossary` | object | 术语表，key 为英文原文，value 为官方中文译名 |
| `pub_date` | string | 发布时间（RFC 3339 格式） |
| `link` | string | 更新公告链接 |

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
5. 调用 LLM 翻译（标准 Function Calling 流程）：
   a. LLM 通过 search_po_terms 工具提取并查询术语
   b. 将工具调用结果以 tool message 归还
   c. LLM 在同一上下文中生成最终译文
          ↓
6. 组合 UpdateNotification（原文 + 译文 + 术语表 + 版本信息）
          ↓
7. PUBLISH 到 Redis 通道
          ↓
8. 在 Redis 中标记该 build 已处理
```

### 下游消费

下游服务通过 Redis `SUBSCRIBE` 命令订阅 `dst-updates` 通道即可接收更新通知，例如：

- 机器人推送（Discord / QQ / 微信等）
- 网站内容更新
- 通知邮件发送
