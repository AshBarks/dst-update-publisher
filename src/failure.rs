//! 失败状态机（DESIGN ADR A5、implementation/02 Step 4）。
//!
//! 状态存 Redis Hash `dst:failure_state`（fields: `count`/`first_ts`/`escalated`）。
//! 决策逻辑 `decide` 为纯函数（便于单测）；`handle_failure` 只在链上任意阶段
//! 失败时被调用，其自身的 Redis 失败只记日志，绝不让状态机成为新的故障源。

use chrono::{DateTime, Utc};
use redis::aio::MultiplexedConnection;

use canonical_msg::{CanonicalMessage, MsgKind};

use crate::error::AppError;
use crate::models::{AppConfig, klei_source};
use crate::publisher::publish_canonical;

pub const FAILURE_STATE_KEY: &str = "dst:failure_state";

const FAILURE_COUNT_FIELD: &str = "count";
const FAILURE_FIRST_TS_FIELD: &str = "first_ts";
const FAILURE_ESCALATED_FIELD: &str = "escalated";

const ALERT_DEDUP_PREFIX: &str = "klei-dst-rss:alert:";
const ALERT_TITLE: &str = "DST 更新抓取失败";
const ALERT_ESCALATED_SUFFIX: &str = "（持续失败升级）";
const ALERT_ESCALATED_TAG: &str = "alert:escalated";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertDecision {
    First,
    Escalate,
    Silent,
}

/// 失败告警决策（纯函数）：
/// - count == 1 → First（首发告警）
/// - count >= escalate_count 或 now - first_ts >= escalate_hours*3600 → Escalate（升级）
/// - 否则 → Silent（静默重试，仅记日志）
pub fn decide(
    count: u32,
    first_ts: i64,
    now: i64,
    escalate_count: u32,
    escalate_hours: i64,
) -> AlertDecision {
    if count == 1 {
        return AlertDecision::First;
    }
    let by_count = count >= escalate_count;
    let by_time = now.saturating_sub(first_ts) >= escalate_hours.saturating_mul(3600);
    if by_count || by_time {
        AlertDecision::Escalate
    } else {
        AlertDecision::Silent
    }
}

/// 构造 kind=ALERT 的 canonical 消息（首发/升级共用；升级带标题后缀与 `alert:escalated` tag）。
/// `first_ts` 决定 dedup_key 中的 UTC 日期；`now_ts` 为 published_at。
pub fn build_alert_message(
    first_ts: i64,
    now_ts: i64,
    stage: &str,
    error_summary: &str,
    failure_count: u64,
    escalated: bool,
) -> CanonicalMessage {
    let first_dt = DateTime::from_timestamp(first_ts, 0);
    let dedup_date = first_dt
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string());
    let first_time = first_dt
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "unknown".to_string());

    let title = if escalated {
        format!("{ALERT_TITLE}{ALERT_ESCALATED_SUFFIX}")
    } else {
        ALERT_TITLE.to_string()
    };

    let body = format!(
        "**{ALERT_TITLE}**\n\n- 失败阶段：{stage}\n- 错误摘要：{error_summary}\n- 累计失败次数：{failure_count}\n- 首次失败时间：{first_time}"
    );

    let mut tags = Vec::new();
    if escalated {
        tags.push(ALERT_ESCALATED_TAG.to_string());
    }

    CanonicalMessage {
        id: ulid::Ulid::generate().to_string(),
        source: Some(klei_source()),
        dedup_key: format!("{ALERT_DEDUP_PREFIX}{dedup_date}"),
        kind: MsgKind::Alert as i32,
        title,
        body_markdown: body.clone(),
        body_text: body,
        links: vec![],
        tags,
        published_at: Some(canonical_msg::pbjson_types::Timestamp {
            seconds: now_ts,
            nanos: 0,
        }),
        domain: None,
    }
}

/// 链上任意阶段失败时调用：递增计数、记录首次失败时间、按决策发布告警到 canonical stream。
/// 自身失败不向外传播（只记日志），避免状态机成为新的故障源。
pub async fn handle_failure(
    stage: &str,
    err: &AppError,
    conn: &mut MultiplexedConnection,
    config: &AppConfig,
) {
    let now = Utc::now().timestamp();

    let bookkeeping: Result<(), AppError> = async {
        let count: i64 = redis::cmd("HINCRBY")
            .arg(FAILURE_STATE_KEY)
            .arg(FAILURE_COUNT_FIELD)
            .arg(1)
            .query_async(conn)
            .await?;

        let first_ts = if count == 1 {
            redis::cmd("HSET")
                .arg(FAILURE_STATE_KEY)
                .arg(FAILURE_FIRST_TS_FIELD)
                .arg(now)
                .arg(FAILURE_ESCALATED_FIELD)
                .arg("0")
                .exec_async(conn)
                .await?;
            now
        } else {
            let stored: Option<i64> = redis::cmd("HGET")
                .arg(FAILURE_STATE_KEY)
                .arg(FAILURE_FIRST_TS_FIELD)
                .query_async(conn)
                .await?;
            stored.unwrap_or(now)
        };

        let decision = decide(
            count as u32,
            first_ts,
            now,
            config.failure_escalate_count,
            config.failure_escalate_hours,
        );

        let already_escalated: bool = redis::cmd("HGET")
            .arg(FAILURE_STATE_KEY)
            .arg(FAILURE_ESCALATED_FIELD)
            .query_async::<Option<String>>(conn)
            .await?
            .as_deref()
            == Some("1");

        match decision {
            AlertDecision::First => {
                let alert = build_alert_message(
                    first_ts,
                    now,
                    stage,
                    &err.to_string(),
                    count as u64,
                    false,
                );
                publish_canonical(conn, &config.redis_msgs_stream, &alert).await?;
            }
            AlertDecision::Escalate if !already_escalated => {
                let alert =
                    build_alert_message(first_ts, now, stage, &err.to_string(), count as u64, true);
                publish_canonical(conn, &config.redis_msgs_stream, &alert).await?;
                redis::cmd("HSET")
                    .arg(FAILURE_STATE_KEY)
                    .arg(FAILURE_ESCALATED_FIELD)
                    .arg("1")
                    .exec_async(conn)
                    .await?;
            }
            _ => {}
        }

        Ok(())
    }
    .await;

    match bookkeeping {
        Ok(()) => tracing::error!("pipeline failed at stage '{}': {}", stage, err),
        Err(e) => tracing::error!(
            "pipeline failed at stage '{}': {}; additionally failure-state bookkeeping failed: {}",
            stage,
            err,
            e
        ),
    }
}
