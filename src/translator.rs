use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessageArgs,
        ChatCompletionTool, ChatCompletionTools, CreateChatCompletionRequest,
        CreateChatCompletionRequestArgs, CreateChatCompletionResponse, FunctionObjectArgs,
    },
};
use serde_json::json;
use std::time::Duration;

use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, PoSearchResult, TranslatedAnnouncement};
use crate::po_search::PoFileIndex;

const SYSTEM_PROMPT: &str = r#"\
You are a professional localizer for the game "Don't Starve Together".
Your job is to translate English update patch notes into Simplified Chinese.

Guidelines:
1. Translation must be accurate, fluent, and match the tone of game announcements.
2. An official glossary is provided in the user message. For every term listed there, use the given Chinese translation EXACTLY as provided: do not translate it yourself, do not alter any character, and do not append the English original or any parenthetical gloss. Every glossary term that occurs in the source MUST appear verbatim in your translation — omitting one is a translation error.
3. You may call `search_po_terms` for game terms that are NOT covered by the glossary, BEFORE translating them. If a tool result gives an official translation, use it exactly.
4. The input is Markdown when it contains Markdown markers (headings starting with `#`, unordered list items starting with `-`, ordered list items, `**bold**`, `[links](url)`). Translate ONLY the human-readable text and preserve ALL Markdown markers, line breaks, blank lines, list item boundaries and list nesting exactly as given. Do not add, remove, merge or split any Markdown structure.
5. Output Simplified Chinese only. Do NOT keep English words or add English in parentheses. Exceptions only: URLs, version numbers (e.g. 3.0), prices, and brand names with no accepted Chinese form (e.g. Twitch).
6. Output ONLY the final Chinese translation, no extra commentary.
"#;

fn build_user_prompt(body: &str, glossary_block: &str) -> String {
    let mut prompt = String::from(
        "Translate the following Don't Starve Together update notes (Markdown) into Chinese.
Preserve the Markdown structure exactly.

",
    );
    if !glossary_block.is_empty() {
        prompt.push_str(glossary_block);
        prompt.push('\n');
    }
    prompt.push_str("Patch notes:\n");
    prompt.push_str(body);
    prompt
}

/// Markdown 块结构签名：`(标题行数, 列表项行数)`。用于校验 LLM 译文没有增删
/// 标题或列表条目（结构被破坏只记 WARN，不阻断发布）。
fn block_signature(markdown: &str) -> (usize, usize) {
    let mut headings = 0;
    let mut items = 0;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            headings += 1;
            continue;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            items += 1;
            continue;
        }
        let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && trimmed[digits..].starts_with(". ") {
            items += 1;
        }
    }
    (headings, items)
}

fn warn_on_structure_change(source: &str, translated: &str) {
    let (source_headings, source_items) = block_signature(source);
    let (translated_headings, translated_items) = block_signature(translated);
    if source_headings != translated_headings || source_items != translated_items {
        tracing::warn!(
            source_headings,
            translated_headings,
            source_items,
            translated_items,
            "translated markdown structure differs from source; heading/list layout may be degraded"
        );
    }
}

pub fn create_llm_client(config: &AppConfig) -> Client<OpenAIConfig> {
    let openai_config = OpenAIConfig::new()
        .with_api_base(&config.llm_api_base)
        .with_api_key(&config.llm_api_key);

    Client::with_config(openai_config)
}

/// 单次 LLM 请求的超时与重试。
///
/// 模型网关偶发挂死（实测 Qwen3.5-4B 首轮 7 分钟无响应），没有超时会把整条
/// 管线拖到失败状态机都触发不了。这里对每次请求做 `timeout`，只对可重试错误
/// （超时/连接错误/408/409/425/429/5xx）做指数退避重试；参数错误等直接失败。
async fn create_chat_with_retry<F>(
    client: &Client<OpenAIConfig>,
    mut build_request: F,
    timeout: Duration,
    max_retries: u32,
    backoff: Duration,
) -> AppResult<CreateChatCompletionResponse>
where
    F: FnMut() -> AppResult<CreateChatCompletionRequest>,
{
    let mut attempt = 0u32;
    loop {
        let request = build_request()?;
        let failure: String =
            match tokio::time::timeout(timeout, client.chat().create(request)).await {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(error)) => {
                    if !is_retryable_llm_error(&error) {
                        return Err(AppError::LlmApi(format!("LLM API call failed: {error}")));
                    }
                    error.to_string()
                }
                Err(_) => format!("request timed out after {}s", timeout.as_secs()),
            };

        if attempt >= max_retries {
            return Err(AppError::LlmApi(format!(
                "LLM API call failed after {} attempts: {failure}",
                attempt + 1
            )));
        }

        attempt += 1;
        let delay = backoff.saturating_mul(2u32.pow((attempt - 1).min(5)));
        tracing::warn!(
            attempt,
            max_retries,
            delay_ms = delay.as_millis() as u64,
            error = %failure,
            "LLM request failed; retrying"
        );
        tokio::time::sleep(delay).await;
    }
}

/// 只重试瞬时错误；鉴权/参数错误重试没有意义。
fn is_retryable_llm_error(error: &OpenAIError) -> bool {
    match error {
        OpenAIError::Reqwest(_) | OpenAIError::StreamError(_) => true,
        OpenAIError::ApiError(response) => is_retryable_status(response.status_code.as_u16()),
        _ => false,
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429) || status >= 500
}

fn build_tools() -> AppResult<Vec<ChatCompletionTools>> {
    let tool = ChatCompletionTool {
        function: FunctionObjectArgs::default()
            .name("search_po_terms")
            .description("查询游戏术语的官方中文译名。在游戏 PO 本地化文件中搜索指定英文术语，返回匹配的中文翻译条目。")
            .parameters(json!({
                "type": "object",
                "properties": {
                    "terms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "需要查询的英文术语列表，可同时查询多个术语"
                    }
                },
                "required": ["terms"]
            }))
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build function object: {}", e)))?,
    };

    Ok(vec![ChatCompletionTools::Function(tool)])
}

fn execute_tool_call(
    tool_call: &ChatCompletionMessageToolCall,
    po_index: &PoFileIndex,
) -> AppResult<(Vec<PoSearchResult>, String)> {
    let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
        .map_err(|e| AppError::LlmResponseParse(format!("failed to parse tool args: {}", e)))?;

    let terms: Vec<&str> = args["terms"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
        .unwrap_or_default();

    let search_results = po_index.search_terms(&terms);

    let tool_output = serde_json::to_string(&search_results).map_err(AppError::Serialization)?;

    Ok((search_results, tool_output))
}

pub async fn full_translate(
    client: &Client<OpenAIConfig>,
    config: &AppConfig,
    announcement: &str,
    po_index: &PoFileIndex,
) -> AppResult<TranslatedAnnouncement> {
    let tools = build_tools()?;

    // 译前确定性地从源文提取官方术语，避免模型“只查自己想查的词”。
    let glossary = crate::glossary::collect(announcement, po_index);
    let glossary_block = crate::glossary::prompt_block(&glossary);
    tracing::info!(
        terms = glossary.len(),
        prompt_chars = glossary_block.len(),
        "extracted glossary terms from source"
    );

    let mut messages: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestSystemMessageArgs::default()
            .content(SYSTEM_PROMPT)
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build system message: {}", e)))?
            .into(),
        ChatCompletionRequestUserMessageArgs::default()
            .content(build_user_prompt(announcement, &glossary_block))
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build user message: {}", e)))?
            .into(),
    ];

    tracing::info!("calling LLM with search_po_terms tool to extract terms");

    let first_response = create_chat_with_retry(
        client,
        || {
            CreateChatCompletionRequestArgs::default()
                .model(&config.llm_model)
                .temperature(config.llm_temperature)
                .messages(messages.clone())
                .tools(tools.clone())
                .build()
                .map_err(|e| AppError::LlmApi(format!("failed to build request: {}", e)))
        },
        Duration::from_secs(config.llm_request_timeout_secs),
        config.llm_max_retries,
        Duration::from_millis(config.llm_retry_backoff_ms),
    )
    .await?;

    let first_choice = first_response
        .choices
        .first()
        .ok_or(AppError::LlmResponseParse(
            "no choices in response".to_string(),
        ))?;

    let first_message = &first_choice.message;

    let mut search_results_collected: Vec<PoSearchResult> = Vec::new();

    let tool_calls = first_message.tool_calls.clone();

    // 0.42：响应里的 tool_calls 是 enum（Function/Custom），这里只处理 function 型调用，
    // 同时把 enum 列表原样回传给 assistant 消息（保持上下文逐字回显）。
    let fn_tool_calls: Vec<ChatCompletionMessageToolCall> = tool_calls
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tc| match tc {
            ChatCompletionMessageToolCalls::Function(c) => Some(c),
            _ => None,
        })
        .collect();

    if let Some(tool_calls) = tool_calls {
        let assistant_msg = ChatCompletionRequestAssistantMessageArgs::default()
            .tool_calls(tool_calls)
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build assistant message: {}", e)))?;
        messages.push(assistant_msg.into());

        for tool_call in &fn_tool_calls {
            let function_name = &tool_call.function.name;

            tracing::info!(
                "LLM called tool: {} with args: {}",
                function_name,
                tool_call.function.arguments
            );

            if function_name == "search_po_terms" {
                let (search_results, tool_output) = execute_tool_call(tool_call, po_index)?;

                search_results_collected.extend(search_results);

                tracing::info!(
                    "got {} search result entries for tool_call_id={}",
                    search_results_collected.len(),
                    tool_call.id
                );

                let tool_msg = ChatCompletionRequestToolMessageArgs::default()
                    .tool_call_id(&tool_call.id)
                    .content(ChatCompletionRequestToolMessageContent::Text(tool_output))
                    .build()
                    .map_err(|e| {
                        AppError::LlmApi(format!("failed to build tool message: {}", e))
                    })?;
                messages.push(tool_msg.into());
            }
        }

        tracing::info!(
            "total {} search results, calling LLM for final translation in same context",
            search_results_collected.len()
        );

        // 长文本下模型可能返回空内容（HTTP 200 但 content 为空），这里最多重试 3 次
        let mut translated_text = String::new();
        let mut finish_note = String::new();

        for attempt in 1..=3 {
            let second_response = create_chat_with_retry(
                client,
                || {
                    CreateChatCompletionRequestArgs::default()
                        .model(&config.llm_model)
                        .temperature(config.llm_temperature)
                        .messages(messages.clone())
                        .build()
                        .map_err(|e| AppError::LlmApi(format!("failed to build request: {}", e)))
                },
                Duration::from_secs(config.llm_request_timeout_secs),
                config.llm_max_retries,
                Duration::from_millis(config.llm_retry_backoff_ms),
            )
            .await?;

            let second_choice =
                second_response
                    .choices
                    .first()
                    .ok_or(AppError::LlmResponseParse(
                        "no choices in final response".to_string(),
                    ))?;

            translated_text = second_choice.message.content.clone().unwrap_or_default();
            finish_note = second_choice
                .finish_reason
                .as_ref()
                .map(|r| format!("{:?}", r))
                .unwrap_or_else(|| "unknown".to_string());

            if !translated_text.trim().is_empty() {
                break;
            }

            tracing::warn!(
                "attempt {}/3: LLM returned empty translation (finish_reason={})",
                attempt,
                finish_note
            );
        }

        if translated_text.trim().is_empty() {
            return Err(AppError::LlmResponseParse(format!(
                "LLM returned empty translation after 3 attempts (finish_reason={})",
                finish_note
            )));
        }

        let translated_text = finalize_translation(translated_text, announcement, &glossary);

        // 调试观察：打印术语检索候选集（模型主动查询的结果，非术语表）
        if !search_results_collected.is_empty() {
            match serde_json::to_string_pretty(&search_results_collected) {
                Ok(pretty) => tracing::debug!("search results preview:\n{}", pretty),
                Err(e) => tracing::debug!("failed to serialize search results preview: {}", e),
            }
        }

        Ok(TranslatedAnnouncement {
            original_text: announcement.to_string(),
            translated_text,
            search_results: search_results_collected,
        })
    } else {
        tracing::info!("LLM responded without tool calls, using direct response as translation");

        let translated_text = first_message.content.clone().unwrap_or_default();
        if translated_text.trim().is_empty() {
            return Err(AppError::LlmResponseParse(
                "LLM responded without tool calls and with empty content".to_string(),
            ));
        }

        let translated_text = finalize_translation(translated_text, announcement, &glossary);

        Ok(TranslatedAnnouncement {
            original_text: announcement.to_string(),
            translated_text,
            search_results: search_results_collected,
        })
    }
}

/// 译文收尾：删术语表英文括注、清 HTML 实体、结构校验、术语缺失告警。
fn finalize_translation(
    translated: String,
    announcement: &str,
    glossary: &[crate::glossary::GlossaryEntry],
) -> String {
    let cleaned = crate::glossary::strip_glossary_glosses(&translated, glossary);
    let cleaned = crate::content::decode_html_entities(&cleaned);
    // 极端情况下括注清理可能把整段清空，此时保留原译文交给上层报错/发布。
    let cleaned = if cleaned.trim().is_empty() {
        translated
    } else {
        cleaned
    };

    warn_on_structure_change(announcement, &cleaned);

    let missing = crate::glossary::missing_terms(&cleaned, glossary);
    if !missing.is_empty() {
        let preview: Vec<&str> = missing.iter().take(20).copied().collect();
        tracing::warn!(
            missing_count = missing.len(),
            glossary_total = glossary.len(),
            terms = ?preview,
            "translated text is missing official glossary terms"
        );
    }

    cleaned
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn classifies_retryable_statuses() {
        for status in [408, 409, 425, 429, 500, 502, 503, 504] {
            assert!(is_retryable_status(status), "{status} should be retryable");
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(
                !is_retryable_status(status),
                "{status} should not be retryable"
            );
        }
    }

    /// 第一次 500、第二次 200：验证退避重试后成功，且确实发了两次请求。
    #[tokio::test]
    async fn retries_transient_error_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let base_url = format!("{}/v1", server.url());

        let failure = server
            .mock("POST", "/v1/chat/completions")
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"boom","type":"server_error"}}"#)
            .expect(1)
            .create_async()
            .await;
        let success = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"x","object":"chat.completion","created":0,"model":"test","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_base(base_url)
                .with_api_key("test-key"),
        );

        let response = create_chat_with_retry(
            &client,
            || {
                CreateChatCompletionRequestArgs::default()
                    .model("test")
                    .messages(vec![
                        ChatCompletionRequestUserMessageArgs::default()
                            .content("hi")
                            .build()
                            .unwrap()
                            .into(),
                    ])
                    .build()
                    .map_err(|e| AppError::LlmApi(e.to_string()))
            },
            Duration::from_secs(5),
            2,
            Duration::from_millis(10),
        )
        .await
        .unwrap();

        assert_eq!(response.choices[0].message.content.as_deref(), Some("ok"));
        failure.assert_async().await;
        success.assert_async().await;
    }

    /// 服务端只接受连接不响应：超时必须生效并返回错误，而不是挂死。
    #[tokio::test]
    async fn times_out_when_server_never_responds() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_base(format!("http://{addr}/v1"))
                .with_api_key("test-key"),
        );

        let started = std::time::Instant::now();
        let error = create_chat_with_retry(
            &client,
            || {
                CreateChatCompletionRequestArgs::default()
                    .model("test")
                    .messages(vec![
                        ChatCompletionRequestUserMessageArgs::default()
                            .content("hi")
                            .build()
                            .unwrap()
                            .into(),
                    ])
                    .build()
                    .map_err(|e| AppError::LlmApi(e.to_string()))
            },
            Duration::from_millis(200),
            0,
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("timed out"),
            "unexpected error: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout did not cancel the request in time"
        );
    }
}
