use dst_update_publisher::cli::{CliArgs, RunMode};
use dst_update_publisher::config::load_config;
use dst_update_publisher::error::{AppError, AppResult};
use dst_update_publisher::models::{AppConfig, ProcessOutcome, ReleaseChannel, UpdateNotification};
use dst_update_publisher::po_search::{PoFileIndex, load_po_index};
use dst_update_publisher::publisher::{
    connect_redis, is_build_processed, mark_build_processed, publish_update,
};
use dst_update_publisher::rss::{fetch_rss_updates, get_latest_pc_update};
use dst_update_publisher::translator::{create_llm_client, full_translate};
use dst_update_publisher::update_page::{fetch_update_page, get_version_info};

use clap::Parser;
use tracing_subscriber::EnvFilter;

async fn process_once(
    config: &AppConfig,
    po_index: &PoFileIndex,
    redis_conn: &mut redis::aio::MultiplexedConnection,
) -> AppResult<ProcessOutcome> {
    tracing::info!("starting single run processing");

    let rss_items = fetch_rss_updates(config).await?;
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

    if is_build_processed(redis_conn, &config.redis_dedupe_key, &rss_item.build_number).await? {
        tracing::info!(
            "build {} already processed, skipping",
            rss_item.build_number
        );
        return Ok(ProcessOutcome::AlreadyProcessed {
            build_number: rss_item.build_number.clone(),
        });
    }

    let page_data = fetch_update_page(config).await?;
    let page_entry = get_version_info(&page_data, &rss_item.revision, &rss_item.build_number)?;

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
        .map_err(|e| AppError::HtmlParse(format!("failed to convert HTML to text: {}", e)))?;

    tracing::info!(
        "announcement text length: {} chars",
        announcement_text.len()
    );

    let llm_client = create_llm_client(config);

    let translated = full_translate(&llm_client, config, &announcement_text, po_index).await?;

    tracing::info!(translated = translated.translated_text, translated.search_results = ?translated.search_results);

    tracing::info!(
        "translation completed, translated text length: {} chars",
        translated.translated_text.len()
    );

    let notification = UpdateNotification::compose(rss_item, &page_entry, &translated);

    let json = notification.to_json().map_err(AppError::Serialization)?;
    tracing::info!("notification JSON length: {} chars", json.len());

    publish_update(redis_conn, &config.redis_channel, &notification).await?;

    mark_build_processed(redis_conn, &config.redis_dedupe_key, &rss_item.build_number).await?;

    tracing::info!(
        "update published successfully for build {}",
        notification.build_number
    );

    Ok(ProcessOutcome::Published {
        build_number: notification.build_number,
    })
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
            }
        }
        RunMode::Poll { interval_secs } => {
            run_poll_loop(&config, &po_index, &mut redis_conn, interval_secs).await?;
        }
    }

    Ok(())
}
