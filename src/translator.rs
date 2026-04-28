use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionToolArgs,
        ChatCompletionToolType, CreateChatCompletionRequestArgs, FunctionObjectArgs,
    },
};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, PoSearchResult, TranslatedAnnouncement};
use crate::po_search::PoFileIndex;

const SYSTEM_PROMPT: &str = "\
你是一个专业的《饥荒联机版》(Don't Starve Together) 本地化翻译助手。
你的任务是将英文更新公告翻译成简体中文。

要求：
1. 翻译必须准确、通顺，符合游戏公告的语气。
2. 游戏内专有术语、物品名称、技能名称、状态名称等与官方中文翻译完全一致。
3. 你可以使用 `search_po_terms` 工具查询术语的官方中文译名。
   - 在开始翻译前，请先将公告中所有你认为属于游戏专有术语的词或短语提取出来，调用该工具查询。
   - 如果工具返回了多个术语，翻译时务必使用工具给出的翻译。
   - 如果某个术语工具未返回结果，可使用合理的意译并在译文后以括号标注英文原文。
4. 输出为 Markdown 格式，保持分段与列表结构清晰。
5. 最终只输出翻译后的中文公告，不要添加额外解释。";

fn build_user_prompt(body: &str) -> String {
    format!(
        "请将以下《饥荒联机版》更新公告翻译成中文。\n公告内容：\n{}",
        body
    )
}

fn build_augmented_user_prompt(body: &str, tool_results: &str) -> String {
    format!(
        "请将以下《饥荒联机版》更新公告翻译成中文。\n\n以下是工具查询到的术语翻译参考：\n{}\n\n公告内容：\n{}",
        tool_results, body
    )
}

pub fn create_llm_client(config: &AppConfig) -> Client<OpenAIConfig> {
    let openai_config = OpenAIConfig::new()
        .with_api_base(&config.llm_api_base)
        .with_api_key(&config.llm_api_key);

    Client::with_config(openai_config)
}

fn build_tools() -> AppResult<Vec<ChatCompletionTool>> {
    let tool = ChatCompletionToolArgs::default()
        .r#type(ChatCompletionToolType::Function)
        .function(
            FunctionObjectArgs::default()
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
                .map_err(|e| AppError::LlmApi(format!("failed to build function object: {}", e)))?)
        .build()
        .map_err(|e| AppError::LlmApi(format!("failed to build tool: {}", e)))?;

    Ok(vec![tool])
}

pub async fn full_translate(
    client: &Client<OpenAIConfig>,
    config: &AppConfig,
    announcement: &str,
    po_index: &PoFileIndex,
) -> AppResult<TranslatedAnnouncement> {
    let tools = build_tools()?;

    let first_messages: Vec<ChatCompletionRequestMessage> = vec![
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
                .messages(first_messages)
                .tools(tools)
                .build()
                .map_err(|e| AppError::LlmApi(format!("failed to build request: {}", e)))?
                .into(),
        )
        .await
        .map_err(|e| AppError::LlmApi(format!("LLM API call failed: {}", e)))?;

    let first_choice = first_response
        .choices
        .first()
        .ok_or(AppError::LlmResponseParse(
            "no choices in response".to_string(),
        ))?;

    let mut search_results_collected: Vec<PoSearchResult> = Vec::new();
    let mut tool_result_lines: Vec<String> = Vec::new();

    if let Some(tool_calls) = &first_choice.message.tool_calls {
        for tool_call in tool_calls {
            let function_name = &tool_call.function.name;
            let arguments_str = &tool_call.function.arguments;

            tracing::info!(
                "LLM called tool: {} with args: {}",
                function_name,
                arguments_str
            );

            if function_name == "search_po_terms" {
                let args: serde_json::Value = serde_json::from_str(arguments_str).map_err(|e| {
                    AppError::LlmResponseParse(format!("failed to parse tool args: {}", e))
                })?;

                let terms: Vec<&str> = args["terms"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
                    .unwrap_or_default();

                let search_results = po_index.search_terms(&terms);

                for (term, result) in terms.iter().zip(search_results) {
                    if result.candidates.is_empty() {
                        tool_result_lines.push(format!("{} → (未找到官方翻译)", term));
                    } else {
                        let entries: Vec<String> = result
                            .candidates
                            .iter()
                            .map(|c| format!("{} → {}", c.original, c.translation))
                            .collect();
                        tool_result_lines.push(entries.join("\n  "));
                    }
                    search_results_collected.push(result);
                }
            }
        }
    }

    let tool_results_text = tool_result_lines.join("\n");

    tracing::info!(
        "got {} tool result entries, calling LLM for final translation",
        tool_result_lines.len()
    );

    let second_messages: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestSystemMessageArgs::default()
            .content(SYSTEM_PROMPT)
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build system message: {}", e)))?
            .into(),
        ChatCompletionRequestUserMessageArgs::default()
            .content(build_augmented_user_prompt(
                announcement,
                &tool_results_text,
            ))
            .build()
            .map_err(|e| AppError::LlmApi(format!("failed to build user message: {}", e)))?
            .into(),
    ];

    let second_response = client
        .chat()
        .create(
            CreateChatCompletionRequestArgs::default()
                .model(&config.llm_model)
                .messages(second_messages)
                .build()
                .map_err(|e| AppError::LlmApi(format!("failed to build request: {}", e)))?
                .into(),
        )
        .await
        .map_err(|e| AppError::LlmApi(format!("LLM API call failed: {}", e)))?;

    let second_choice = second_response
        .choices
        .first()
        .ok_or(AppError::LlmResponseParse(
            "no choices in final response".to_string(),
        ))?;

    let translated_text = second_choice.message.content.clone().unwrap_or_default();

    Ok(TranslatedAnnouncement {
        original_text: announcement.to_string(),
        translated_text,
        search_results: search_results_collected,
    })
}
