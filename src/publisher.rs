use canonical_msg::prost::Message;
use canonical_msg::CanonicalMessage;

use crate::error::{AppError, AppResult};
use crate::models::AppConfig;

/// canonical 去重键前缀（A13：结构由 A 自定、下游不解析），Phase 4 起成为唯一真相。
/// 与 `models::KLEI_DST_SOURCE_ID` 保持同构：`dedup:{source_id}:{build_number}`。
pub fn canonical_dedup_key(build_number: &str) -> String {
    format!("dedup:klei-dst-rss:{build_number}")
}

pub async fn connect_redis(config: &AppConfig) -> AppResult<redis::aio::MultiplexedConnection> {
    let client = redis::Client::open(config.redis_url.as_str()).map_err(AppError::Redis)?;

    let conn = client
        .get_multiplexed_tokio_connection()
        .await
        .map_err(AppError::Redis)?;

    tracing::info!("connected to Redis at {}", config.redis_url);

    Ok(conn)
}

pub async fn is_build_processed(
    conn: &mut redis::aio::MultiplexedConnection,
    dedupe_key: &str,
    build_number: &str,
) -> AppResult<bool> {
    let result: Option<String> = redis::cmd("GET")
        .arg(dedupe_key)
        .query_async(conn)
        .await
        .map_err(AppError::Redis)?;

    match result {
        Some(stored) => Ok(stored == build_number),
        None => Ok(false),
    }
}

pub async fn mark_build_processed(
    conn: &mut redis::aio::MultiplexedConnection,
    dedupe_key: &str,
    build_number: &str,
) -> AppResult<()> {
    // 判定逻辑不变：is_build_processed 仍只读旧键。新键仅冗余记录，自本批开始积累，
    // Phase 4 dst-qq-push 退役时成为唯一真相（DESIGN §5.2）。
    let canonical_key = canonical_dedup_key(build_number);

    redis::cmd("SET")
        .arg(dedupe_key)
        .arg(build_number)
        .exec_async(conn)
        .await
        .map_err(AppError::Redis)?;

    redis::cmd("SET")
        .arg(&canonical_key)
        .arg(build_number)
        .exec_async(conn)
        .await
        .map_err(AppError::Redis)?;

    tracing::info!(
        "marked build {} as processed in Redis keys '{}' and '{}'",
        build_number,
        dedupe_key,
        canonical_key
    );

    Ok(())
}

/// 发布 canonical 消息到 Redis Stream（proto 二进制载荷进 `data` field，fire-and-forget，
/// 与 PUBLISH 语义对齐；`data` field 名与 streams-lib 的 `DATA_FIELD` 常量一致）。
pub async fn publish_canonical(
    conn: &mut redis::aio::MultiplexedConnection,
    stream: &str,
    message: &CanonicalMessage,
) -> AppResult<()> {
    let bytes = message.encode_to_vec();

    redis::cmd("XADD")
        .arg(stream)
        .arg("*")
        .arg("data")
        .arg(&bytes)
        .exec_async(conn)
        .await
        .map_err(AppError::Redis)?;

    tracing::info!(
        "published canonical message kind={} id={} to Redis stream '{}'",
        message.kind,
        message.id,
        stream
    );

    Ok(())
}
