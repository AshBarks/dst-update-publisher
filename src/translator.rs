use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessageArgs,
        ChatCompletionTool, ChatCompletionTools, CreateChatCompletionRequestArgs,
        FunctionObjectArgs,
    },
};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, PoSearchResult, TranslatedAnnouncement};
use crate::po_search::PoFileIndex;

const SYSTEM_PROMPT: &str = r#"\
You are a professional localizer for the game "Don't Starve Together".
Your job is to translate English update patch notes into Simplified Chinese.

Guidelines:
1. Translation must be accurate, fluent, and match the tone of game announcements.
2. First, extract from the patch notes any words or phrases you think are game terms (items, skills, statuses, character names, etc.). Call `search_po_terms` with those terms BEFORE translating.
3. If a term is found in the tool results, use the provided official translation exactly. If a term is not found in the tool results, translate it sensibly on your own.
4. The input is Markdown when it contains Markdown markers (headings starting with `#`, unordered list items starting with `-`, ordered list items, `**bold**`, `[links](url)`). Translate ONLY the human-readable text and preserve ALL Markdown markers, line breaks, blank lines, list item boundaries and list nesting exactly as given. Do not add, remove, merge or split any Markdown structure.
5. For EVERY term you identified and queried via `search_po_terms`, append the English original in half-width parentheses right after its translation, e.g. "洞穴蠕虫 (Depths Worm)". This is the only addition allowed beyond a faithful translation.
6. Output ONLY the final Chinese translation, no extra commentary.
"#;

fn build_user_prompt(body: &str) -> String {
    format!(
        "Translate the following Don't Starve Together update notes (Markdown) into Chinese.
Preserve the Markdown structure exactly.

Patch notes:
{}",
        body
    )
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

    let mut messages: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestSystemMessageArgs::default()
            .content(SYSTEM_PROMPT)
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build system message: {}", e)))?
            .into(),
        ChatCompletionRequestUserMessageArgs::default()
            .content(build_user_prompt(announcement))
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build user message: {}", e)))?
            .into(),
    ];

    tracing::info!("calling LLM with search_po_terms tool to extract terms");

    let first_response = client
        .chat()
        .create(
            CreateChatCompletionRequestArgs::default()
                .model(&config.llm_model)
                .temperature(config.llm_temperature)
                .messages(messages.clone())
                .tools(tools.clone())
                .build()
                .map_err(|e| AppError::LlmApi(format!("failed to build request: {}", e)))?,
        )
        .await
        .map_err(|e| AppError::LlmApi(format!("LLM API call failed: {}", e)))?;

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
            let second_response = client
                .chat()
                .create(
                    CreateChatCompletionRequestArgs::default()
                        .model(&config.llm_model)
                        .temperature(config.llm_temperature)
                        .messages(messages.clone())
                        .build()
                        .map_err(|e| AppError::LlmApi(format!("failed to build request: {}", e)))?,
                )
                .await
                .map_err(|e| AppError::LlmApi(format!("LLM API call failed: {}", e)))?;

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

        warn_on_structure_change(announcement, &translated_text);

        // 调试观察：打印术语检索候选集（即发布消息中 glossary 字段的内容）
        if !search_results_collected.is_empty() {
            match serde_json::to_string_pretty(&search_results_collected) {
                Ok(pretty) => tracing::debug!("glossary preview:\n{}", pretty),
                Err(e) => tracing::debug!("failed to serialize glossary preview: {}", e),
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

        warn_on_structure_change(announcement, &translated_text);

        Ok(TranslatedAnnouncement {
            original_text: announcement.to_string(),
            translated_text,
            search_results: search_results_collected,
        })
    }
}
