use dst_update_publisher::cli::{CliArgs, RunMode};
use dst_update_publisher::config::load_config;
use dst_update_publisher::error::{AppError, AppResult};
use dst_update_publisher::failure::{FAILURE_STATE_KEY, handle_failure};
use dst_update_publisher::json_translator::translate_json_file;
use dst_update_publisher::models::{AppConfig, ProcessOutcome, ReleaseChannel, build_canonical};
use dst_update_publisher::po_search::{PoFileIndex, load_po_index};
use dst_update_publisher::publisher::{
    connect_redis, is_build_processed, mark_build_processed, publish_canonical,
};
use dst_update_publisher::rss::{fetch_rss_updates, get_latest_pc_update};
use dst_update_publisher::translator::{create_llm_client, full_translate};
use dst_update_publisher::update_page::{fetch_update_page, get_version_info};

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// 记录失败发生的链上阶段，供失败状态机在告警正文中定位（implementation/02 Step 4）。
#[derive(Debug)]
struct StageError {
    stage: &'static str,
    source: AppError,
}

impl StageError {
    fn at(stage: &'static str, source: AppError) -> Self {
        Self { stage, source }
    }
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stage '{}': {}", self.stage, self.source)
    }
}

impl std::error::Error for StageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// 单轮 抓取→去重→版本页→html2text→翻译→canonical 构造→双发→标记。任何阶段失败
/// 以 StageError 返回（由 process_once 转失败状态机），不向上传播（进程不崩）。
async fn run_pipeline(
    config: &AppConfig,
    po_index: &PoFileIndex,
    redis_conn: &mut redis::aio::MultiplexedConnection,
) -> Result<ProcessOutcome, StageError> {
    tracing::info!("starting single run processing");

    let rss_items = fetch_rss_updates(config)
        .await
        .map_err(|e| StageError::at("fetch_rss", e))?;
    let latest = get_latest_pc_update(&rss_items);

    let rss_item = match latest {
        Some(item) => item,
        None => {
            tracing::warn!("no PC update found in RSS feed");
            return Ok(ProcessOutcome::NoUpdateAvailable);
        }
    };

    tracing::info!(
        "latest PC update: build={}, revision={}, date={}",
        rss_item.build_number,
        rss_item.revision,
        rss_item.pub_date
    );

    if is_build_processed(redis_conn, &config.redis_dedupe_key, &rss_item.build_number)
        .await
        .map_err(|e| StageError::at("dedup_check", e))?
    {
        tracing::info!(
            "build {} already processed, skipping",
            rss_item.build_number
        );
        return Ok(ProcessOutcome::AlreadyProcessed {
            build_number: rss_item.build_number.clone(),
        });
    }

    let page_data = fetch_update_page(config)
        .await
        .map_err(|e| StageError::at("fetch_update_page", e))?;
    let page_entry = get_version_info(&page_data, &rss_item.revision, &rss_item.build_number)
        .map_err(|e| StageError::at("parse_update_page", e))?;

    tracing::info!(
        "version info: revision={}, channel={}, hotfix={}",
        page_entry.revision,
        match page_entry.channel {
            ReleaseChannel::Release => "release",
            ReleaseChannel::Beta => "beta",
        },
        page_entry.is_hotfix
    );

    let announcement_text = html2text::from_read(rss_item.description_html.as_bytes(), 80)
        .map_err(|e| {
            StageError::at(
                "html2text",
                AppError::HtmlParse(format!("failed to convert HTML to text: {}", e)),
            )
        })?;

    tracing::info!(
        "announcement text length: {} chars",
        announcement_text.len()
    );

    let llm_client = create_llm_client(config);

    let translated = full_translate(&llm_client, config, &announcement_text, po_index)
        .await
        .map_err(|e| StageError::at("translate", e))?;

    tracing::info!(translated = translated.translated_text, translated.search_results = ?translated.search_results);

    tracing::info!(
        "translation completed, translated text length: {} chars",
        translated.translated_text.len()
    );

    let canonical = build_canonical(rss_item, &page_entry, &announcement_text, &translated)
        .map_err(|e| StageError::at("build_canonical", e))?;

    // 旧格式（UpdateNotification → dst:update/dst-updates）已于 Phase 3 停发：
    // canonical 是唯一线上格式（06 Step 5.4）。XADD 恰好一次：发布失败则不标记
    // 去重（下轮整链重试），与"翻译失败不标 dedup"语义一致。
    publish_canonical(redis_conn, &config.redis_msgs_stream, &canonical)
        .await
        .map_err(|e| StageError::at("publish_canonical", e))?;

    mark_build_processed(redis_conn, &config.redis_dedupe_key, &rss_item.build_number)
        .await
        .map_err(|e| StageError::at("mark_processed", e))?;

    // 成功发布即隐式恢复（DESIGN A5：无恢复消息）。
    if let Err(e) = redis::cmd("DEL")
        .arg(FAILURE_STATE_KEY)
        .exec_async(redis_conn)
        .await
    {
        tracing::warn!(
            "failed to clear failure state after successful publish: {}",
            e
        );
    }

    Ok(ProcessOutcome::Published {
        build_number: canonical
            .dedup_key
            .strip_prefix("klei-dst-rss:")
            .unwrap_or(&canonical.dedup_key)
            .to_string(),
    })
}

async fn process_once(
    config: &AppConfig,
    po_index: &PoFileIndex,
    redis_conn: &mut redis::aio::MultiplexedConnection,
) -> AppResult<ProcessOutcome> {
    match run_pipeline(config, po_index, redis_conn).await {
        Ok(outcome) => Ok(outcome),
        Err(stage_err) => {
            handle_failure(stage_err.stage, &stage_err.source, redis_conn, config).await;
            Ok(ProcessOutcome::Failed {
                reason: stage_err.to_string(),
            })
        }
    }
}

async fn run_poll_loop(
    config: &AppConfig,
    po_index: &PoFileIndex,
    redis_conn: &mut redis::aio::MultiplexedConnection,
    interval_secs: u64,
) -> AppResult<()> {
    loop {
        match process_once(config, po_index, redis_conn).await {
            Ok(ProcessOutcome::Published { build_number }) => {
                tracing::info!("update published successfully for build {}", build_number)
            }
            Ok(ProcessOutcome::AlreadyProcessed { build_number }) => {
                tracing::info!("build {} already processed, will check later", build_number)
            }
            Ok(ProcessOutcome::NoUpdateAvailable) => {
                tracing::info!("no PC update found, will check later")
            }
            Ok(ProcessOutcome::Failed { reason }) => {
                tracing::error!("processing failed: {}", reason)
            }
            Err(e) => tracing::error!("processing failed: {}", e),
        }
        tracing::info!("sleeping for {} seconds before next check", interval_secs);
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
    }
}

#[tokio::main]
async fn main() -> AppResult<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let args = CliArgs::parse();
    let run_mode = args.run_mode();

    tracing::info!("running in mode: {:?}", run_mode);

    let config = load_config()?;
    let po_index = load_po_index(&config.po_zip_path, &config.po_zip_po_files)?;
    let mut redis_conn = connect_redis(&config).await?;

    match run_mode {
        RunMode::Once => {
            let outcome = process_once(&config, &po_index, &mut redis_conn).await?;
            match outcome {
                ProcessOutcome::Published { build_number } => {
                    tracing::info!("update published successfully for build {}", build_number)
                }
                ProcessOutcome::AlreadyProcessed { build_number } => {
                    tracing::info!("build {} already processed, nothing to do", build_number)
                }
                ProcessOutcome::NoUpdateAvailable => {
                    tracing::info!("no PC update found, nothing to do")
                }
                ProcessOutcome::Failed { reason } => {
                    tracing::error!("processing failed: {}", reason)
                }
            }
        }
        RunMode::Poll { interval_secs } => {
            run_poll_loop(&config, &po_index, &mut redis_conn, interval_secs).await?;
        }
        RunMode::TranslateJson {
            input,
            output,
            concurrency,
        } => {
            translate_json_file(&config, &po_index, &input, &output, concurrency).await?;
        }
    }

    Ok(())
}
