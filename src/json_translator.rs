use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use crate::error::AppResult;
use crate::models::{AppConfig, DstUpdatesFile};
use crate::po_search::PoFileIndex;
use crate::translator::{create_llm_client, full_translate};
use futures::stream::{self, StreamExt};
use tokio::sync::{Mutex, Semaphore};

fn write_updates_to_file(updates_file: &DstUpdatesFile, output: &str) -> AppResult<()> {
    let output_path = Path::new(output);
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json_output = serde_json::to_string_pretty(updates_file)?;

    // 先写入临时文件并刷盘，再原子替换目标文件。
    // 直接写目标文件时若进程中途被杀，文件会截断损坏，
    // 导致重启时 merge_with_progress 解析失败、丢失全部翻译进度
    let tmp_path = output_path.with_extension("json.tmp");
    {
        let mut tmp = std::fs::File::create(&tmp_path)?;
        tmp.write_all(json_output.as_bytes())?;
        tmp.sync_all()?;
    }
    std::fs::rename(&tmp_path, output_path)?;
    Ok(())
}

fn merge_with_progress(input: DstUpdatesFile, output: &str) -> DstUpdatesFile {
    let output_path = Path::new(output);
    if !output_path.exists() {
        tracing::info!("no existing output file, starting fresh from input");
        return input;
    }

    let content = match std::fs::read_to_string(output_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "failed to read output file {}: {}, starting fresh",
                output,
                e
            );
            return input;
        }
    };

    let prev: DstUpdatesFile = match serde_json::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "failed to parse output file {}: {}, starting fresh",
                output,
                e
            );
            return input;
        }
    };

    let translated_map: HashMap<u64, String> = prev
        .updates
        .into_iter()
        .filter_map(|e| e.content_text_zh.map(|zh| (e.build_number, zh)))
        .collect();

    if translated_map.is_empty() {
        tracing::info!("output file exists but has no translations, starting fresh from input");
        return input;
    }

    tracing::info!(
        "found {} previously translated entries in output file",
        translated_map.len()
    );

    let mut merged = input;
    for entry in &mut merged.updates {
        if let Some(zh) = translated_map.get(&entry.build_number) {
            entry.content_text_zh = Some(zh.clone());
        }
    }

    merged
}

pub async fn translate_json_file(
    config: &AppConfig,
    po_index: &PoFileIndex,
    input: &str,
    output: &str,
    concurrency: usize,
) -> AppResult<()> {
    let input_path = Path::new(input);
    let content = std::fs::read_to_string(input_path)?;
    let input_file: DstUpdatesFile = serde_json::from_str(&content)?;

    let updates_file = merge_with_progress(input_file, output);
    let shared = Arc::new(Mutex::new(updates_file));

    let total = {
        let guard = shared.lock().await;
        guard.updates.len()
    };

    let already_translated = {
        let guard = shared.lock().await;
        guard
            .updates
            .iter()
            .filter(|u| u.content_text_zh.is_some())
            .count()
    };
    let need_translate = total - already_translated;

    tracing::info!(
        "total: {}, already translated: {}, need translation: {}",
        total,
        already_translated,
        need_translate
    );

    if need_translate == 0 {
        tracing::info!("all entries already translated, writing output and exiting");
        let guard = shared.lock().await;
        write_updates_to_file(&guard, output)?;
        return Ok(());
    }

    {
        let guard = shared.lock().await;
        write_updates_to_file(&guard, output)?;
    }

    tracing::info!("initial state written to {}", output);

    let llm_client = create_llm_client(config);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let output_owned = output.to_string();

    let indices: Vec<usize> = (0..total).collect();

    stream::iter(indices)
        .map(|idx| {
            let shared = shared.clone();
            let llm_client = llm_client.clone();
            let po_index = po_index.clone();
            let semaphore = semaphore.clone();
            let output_owned = output_owned.clone();

            async move {
                let (content_text, build_number, already_done) = {
                    let guard = shared.lock().await;
                    let entry = &guard.updates[idx];
                    let done = entry.content_text_zh.is_some();
                    (entry.content_text.clone(), entry.build_number, done)
                };

                if already_done {
                    tracing::info!(
                        "[{}/{}] build={} already translated, skipping",
                        idx + 1,
                        total,
                        build_number
                    );
                    return;
                }

                let _permit = semaphore.acquire().await.unwrap();

                tracing::info!(
                    "[{}/{}] translating build={} ({} chars)",
                    idx + 1,
                    total,
                    build_number,
                    content_text.len()
                );

                match full_translate(&llm_client, config, &content_text, &po_index).await {
                    Ok(translated) => {
                        let mut guard = shared.lock().await;
                        guard.updates[idx].content_text_zh = Some(translated.translated_text);

                        if let Err(e) = write_updates_to_file(&guard, &output_owned) {
                            tracing::error!(
                                "[{}/{}] build={} failed to write output: {}",
                                idx + 1,
                                total,
                                build_number,
                                e
                            );
                        } else {
                            tracing::info!(
                                "[{}/{}] build={} translation saved",
                                idx + 1,
                                total,
                                build_number
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "[{}/{}] build={} translation failed: {}",
                            idx + 1,
                            total,
                            build_number,
                            e
                        );
                    }
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect::<()>()
        .await;

    let (success_count, fail_count) = {
        let guard = shared.lock().await;
        let success_count = guard
            .updates
            .iter()
            .filter(|u| u.content_text_zh.is_some())
            .count() as u64;
        let fail_count = total as u64 - success_count;
        (success_count, fail_count)
    };

    tracing::info!(
        "translation complete: {} succeeded, {} failed out of {} total",
        success_count,
        fail_count,
        total
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DstUpdateEntry;

    fn make_updates_file(build_number: u64) -> DstUpdatesFile {
        DstUpdatesFile {
            total: 1,
            fetched_at: "2025-01-01T00:00:00Z".into(),
            updates: vec![DstUpdateEntry {
                build_number,
                link: "https://example.com".into(),
                release_date: "2025-01-01".into(),
                is_hotfix: false,
                status: "release".into(),
                is_current: true,
                row_id: 1,
                release_id: 1,
                title: "test".into(),
                content_text: "hello".into(),
                content_text_zh: Some("你好".into()),
            }],
        }
    }

    #[test]
    fn write_is_atomic_and_leaves_no_tmp_file() {
        let dir =
            std::env::temp_dir().join(format!("dst_translator_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("out.json");
        let output_str = output.to_str().unwrap();

        write_updates_to_file(&make_updates_file(42), output_str).unwrap();

        // 目标文件内容正确
        let content = std::fs::read_to_string(&output).unwrap();
        let parsed: DstUpdatesFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.updates[0].build_number, 42);
        assert_eq!(parsed.updates[0].content_text_zh.as_deref(), Some("你好"));

        // 不残留临时文件
        let tmp = output.with_extension("json.tmp");
        assert!(!tmp.exists(), "tmp file should be renamed away");

        // 覆盖写入同样正常（模拟逐条进度落盘）
        write_updates_to_file(&make_updates_file(43), output_str).unwrap();
        let reparsed: DstUpdatesFile =
            serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(reparsed.updates[0].build_number, 43);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
