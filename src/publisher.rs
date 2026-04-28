use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, UpdateNotification};

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
    redis::cmd("SET")
        .arg(dedupe_key)
        .arg(build_number)
        .exec_async(conn)
        .await
        .map_err(AppError::Redis)?;

    tracing::info!(
        "marked build {} as processed in Redis key '{}'",
        build_number,
        dedupe_key
    );

    Ok(())
}

pub async fn publish_update(
    conn: &mut redis::aio::MultiplexedConnection,
    channel: &str,
    notification: &UpdateNotification,
) -> AppResult<()> {
    let json = notification.to_json().map_err(AppError::Serialization)?;

    redis::cmd("PUBLISH")
        .arg(channel)
        .arg(&json)
        .exec_async(conn)
        .await
        .map_err(AppError::Redis)?;

    tracing::info!(
        "published update notification to Redis channel '{}'",
        channel
    );

    Ok(())
}
