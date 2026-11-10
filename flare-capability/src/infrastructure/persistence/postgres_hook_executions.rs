//! `hook_executions`：Hook 执行证据的读与保留期（写入在 Ingest）。
//!
//! 统计与查询都限定租户与 Hook（走 `idx_hook_executions_tenant_hook_executed`，再按 `hook_id` 只留
//! 这一条 Hook 的行：同名删掉重建的 Hook 号不同），计数按 `sample_weight` 加权；保留期清理按执行时间
//! 从旧到新分批删（走 `idx_hook_executions_executed_at`），每批先物化再删，多实例同时清理时互相
//! 跳过已锁的行。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use flare_server_core::error::{ErrorBuilder, ErrorCode, FlareError, Result};
use sqlx::{PgPool, Row};

use crate::domain::model::{
    HOOK_CIRCUIT_OPEN_CODE, HookExecutionFilter, HookExecutionScope, HookExecutionStatistics,
    HookExecutionTimeRange, StoredHookExecution,
};
use crate::domain::repository::HookExecutionRepository;

/// 统计里的加权 p99：按耗时排序累加权重，取累计权重首次达到总权重 99% 的耗时。熔断短路没有调用
/// Hook，不计入耗时。
const STATISTICS_SQL: &str = "
WITH scoped AS (
    SELECT success, latency_ms, error_code, sample_weight::int8 AS w
      FROM hook_executions
     WHERE ($1::TEXT IS NULL OR tenant_id = $1) AND hook_type = $2 AND hook_name = $3
       AND hook_id = $7 AND executed_at >= $4 AND executed_at < $5
), called AS (
    SELECT latency_ms,
           sum(w) OVER (ORDER BY latency_ms ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS cum,
           sum(w) OVER () AS called_total,
           w
      FROM scoped
     WHERE latency_ms IS NOT NULL AND error_code IS DISTINCT FROM $6
)
SELECT
    (SELECT COALESCE(sum(w), 0) FROM scoped)::int8 AS total,
    (SELECT COALESCE(sum(w) FILTER (WHERE success), 0) FROM scoped)::int8 AS succeeded,
    (SELECT COALESCE(sum(w) FILTER (WHERE NOT success), 0) FROM scoped)::int8 AS failed,
    (SELECT COALESCE(sum(w) FILTER (WHERE error_code = $6), 0) FROM scoped)::int8 AS circuit_open,
    (SELECT sum(latency_ms::float8 * w) / NULLIF(sum(w), 0) FROM called)::float8 AS avg_latency,
    (SELECT min(latency_ms) FROM called WHERE cum >= 0.99 * called_total)::float8 AS p99_latency";

const ERROR_CODES_SQL: &str = "
SELECT error_code, sum(sample_weight)::int8 AS count
  FROM hook_executions
 WHERE ($1::TEXT IS NULL OR tenant_id = $1) AND hook_type = $2 AND hook_name = $3
   AND hook_id = $6 AND executed_at >= $4 AND executed_at < $5
   AND NOT success AND error_code IS NOT NULL
 GROUP BY error_code";

/// 这一批先在物化的 CTE 里选定并加锁，只算一次。不能写成 `WHERE execution_id IN (SELECT ...
/// LIMIT .. FOR UPDATE)`：规划器可把它做成每个外层行重扫一次的半连接，重扫时跳过本语句已删的行、
/// 换下一批，一条语句就删光全部过期行，批大小形同虚设。
const DELETE_SQL: &str = "
WITH batch AS MATERIALIZED (
    SELECT execution_id FROM hook_executions
     WHERE executed_at < $1
     ORDER BY executed_at
     LIMIT $2
       FOR UPDATE SKIP LOCKED
)
DELETE FROM hook_executions AS h
 USING batch
 WHERE h.execution_id = batch.execution_id";

const QUERY_SQL: &str = "
SELECT execution_id, message_id, success, latency_ms, error_code, error_message, executed_at,
       sample_weight
  FROM hook_executions
 WHERE ($1::TEXT IS NULL OR tenant_id = $1) AND hook_type = $2 AND hook_name = $3
   AND hook_id = $9 AND executed_at >= $4 AND executed_at < $5
   AND ($6::text IS NULL OR message_id = $6)
   AND (NOT $7 OR success)
 ORDER BY executed_at DESC, execution_id DESC
 LIMIT $8";

/// `hook_executions` 仓储（与 Hook 配置共用连接池）。
pub struct PostgresHookExecutionRepository {
    pool: Arc<PgPool>,
}

impl PostgresHookExecutionRepository {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }
}

/// 存储错误只带固定说明与操作名；连接串、SQL 与行内容不进错误文本。
fn storage_error(operation: &'static str, error: sqlx::Error) -> FlareError {
    tracing::warn!(operation, error = %error, "hook execution storage failed");
    ErrorBuilder::new(
        ErrorCode::DatabaseError,
        "hook execution storage is unavailable",
    )
    .details(operation)
    .build_error()
}

/// `SystemTime` 换成库里的时间戳（保留到纳秒，库存微秒）。chrono 的 `From<SystemTime>` 越界直接
/// panic，这里越界返回无效参数：管理接口的时间在领域层已限到 9999 年末，这是第二道防线。
fn utc(time: SystemTime) -> Result<DateTime<Utc>> {
    let converted = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs())
            .ok()
            .and_then(|secs| DateTime::from_timestamp(secs, after.subsec_nanos())),
        Err(before) => {
            let before = before.duration();
            i64::try_from(before.as_secs())
                .ok()
                .and_then(|secs| match before.subsec_nanos() {
                    0 => DateTime::from_timestamp(secs.checked_neg()?, 0),
                    nanos => DateTime::from_timestamp(
                        secs.checked_neg()?.checked_sub(1)?,
                        1_000_000_000 - nanos,
                    ),
                })
        }
    };
    converted.ok_or_else(|| {
        ErrorBuilder::new(ErrorCode::InvalidParameter, "time is out of range").build_error()
    })
}

#[async_trait::async_trait]
impl HookExecutionRepository for PostgresHookExecutionRepository {
    async fn delete_executed_before(&self, cutoff: SystemTime, limit: u32) -> Result<u64> {
        let result = sqlx::query(DELETE_SQL)
            .bind(utc(cutoff)?)
            .bind(i64::from(limit.max(1)))
            .execute(self.pool.as_ref())
            .await
            .map_err(|e| storage_error("delete_expired", e))?;
        Ok(result.rows_affected())
    }

    async fn statistics(
        &self,
        scope: &HookExecutionScope,
        range: &HookExecutionTimeRange,
    ) -> Result<HookExecutionStatistics> {
        let row = sqlx::query(STATISTICS_SQL)
            .bind(&scope.tenant_id)
            .bind(&scope.hook_type)
            .bind(&scope.hook_name)
            .bind(utc(range.from)?)
            .bind(utc(range.to)?)
            .bind(HOOK_CIRCUIT_OPEN_CODE)
            .bind(&scope.hook_id)
            .fetch_one(self.pool.as_ref())
            .await
            .map_err(|e| storage_error("statistics", e))?;
        let codes = sqlx::query(ERROR_CODES_SQL)
            .bind(&scope.tenant_id)
            .bind(&scope.hook_type)
            .bind(&scope.hook_name)
            .bind(utc(range.from)?)
            .bind(utc(range.to)?)
            .bind(&scope.hook_id)
            .fetch_all(self.pool.as_ref())
            .await
            .map_err(|e| storage_error("statistics_codes", e))?;
        let mut error_count_by_code = HashMap::with_capacity(codes.len());
        for code in codes {
            error_count_by_code.insert(
                code.try_get::<String, _>("error_code")
                    .map_err(|e| storage_error("statistics_codes", e))?,
                code.try_get::<i64, _>("count")
                    .map_err(|e| storage_error("statistics_codes", e))?,
            );
        }
        let read = |column: &str| -> Result<i64> {
            row.try_get::<i64, _>(column)
                .map_err(|e| storage_error("statistics", e))
        };
        let float = |column: &str| -> Result<f64> {
            Ok(row
                .try_get::<Option<f64>, _>(column)
                .map_err(|e| storage_error("statistics", e))?
                .unwrap_or(0.0))
        };
        Ok(HookExecutionStatistics {
            total_executions: read("total")?,
            success_count: read("succeeded")?,
            failure_count: read("failed")?,
            avg_latency_ms: float("avg_latency")?,
            p99_latency_ms: float("p99_latency")?,
            circuit_open_count: read("circuit_open")?,
            error_count_by_code,
        })
    }

    async fn query(&self, filter: &HookExecutionFilter) -> Result<Vec<StoredHookExecution>> {
        let rows = sqlx::query(QUERY_SQL)
            .bind(&filter.scope.tenant_id)
            .bind(&filter.scope.hook_type)
            .bind(&filter.scope.hook_name)
            .bind(utc(filter.range.from)?)
            .bind(utc(filter.range.to)?)
            .bind(filter.message_id.as_deref())
            .bind(filter.success_only)
            .bind(i64::from(filter.limit))
            .bind(&filter.scope.hook_id)
            .fetch_all(self.pool.as_ref())
            .await
            .map_err(|e| storage_error("query", e))?;
        let mut executions = Vec::with_capacity(rows.len());
        for row in rows {
            let executed_at: DateTime<Utc> = row
                .try_get("executed_at")
                .map_err(|e| storage_error("query", e))?;
            executions.push(StoredHookExecution {
                execution_id: row
                    .try_get("execution_id")
                    .map_err(|e| storage_error("query", e))?,
                message_id: row
                    .try_get("message_id")
                    .map_err(|e| storage_error("query", e))?,
                success: row
                    .try_get("success")
                    .map_err(|e| storage_error("query", e))?,
                latency_ms: row
                    .try_get("latency_ms")
                    .map_err(|e| storage_error("query", e))?,
                error_code: row
                    .try_get("error_code")
                    .map_err(|e| storage_error("query", e))?,
                error_message: row
                    .try_get("error_message")
                    .map_err(|e| storage_error("query", e))?,
                executed_at: SystemTime::from(executed_at),
                sample_weight: u32::try_from(
                    row.try_get::<i32, _>("sample_weight")
                        .map_err(|e| storage_error("query", e))?,
                )
                .unwrap_or(1),
            });
        }
        Ok(executions)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// 可表示的时间与 chrono 自己的换算逐纳秒一致（含纪元之前）；越界的返回无效参数，不 panic。
    #[test]
    fn timestamps_convert_exactly_and_out_of_range_times_are_rejected() {
        let epoch = SystemTime::UNIX_EPOCH;
        for time in [
            epoch,
            SystemTime::now(),
            epoch + Duration::new(253_402_300_799, 999_999_999),
            epoch - Duration::new(86_400, 0),
            epoch - Duration::new(86_400, 250),
        ] {
            assert_eq!(utc(time).unwrap(), DateTime::<Utc>::from(time), "{time:?}");
        }
        for huge in [
            epoch + Duration::from_millis(9_000_000_000_000_000),
            epoch + Duration::from_secs(i64::MAX as u64),
            epoch - Duration::from_secs(1 << 60),
        ] {
            let error = utc(huge).unwrap_err();
            assert_eq!(error.code(), Some(ErrorCode::InvalidParameter), "{huge:?}");
        }
    }

    /// 绕过领域层直接给越界的时间：统计与查询回无效参数，不 panic，也不发查询（连接池是惰性的，
    /// 指向的端口没有库）。
    #[tokio::test]
    async fn out_of_range_times_fail_before_any_query() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://flare@127.0.0.1:1/none")
            .unwrap();
        let repository = PostgresHookExecutionRepository::new(Arc::new(pool));
        let scope = HookExecutionScope {
            tenant_id: Some("t1".into()),
            hook_type: "pre_send".into(),
            hook_name: "dlp".into(),
            hook_id: "7".into(),
        };
        let range = HookExecutionTimeRange {
            from: SystemTime::UNIX_EPOCH,
            to: SystemTime::UNIX_EPOCH + Duration::from_millis(9_000_000_000_000_000),
        };
        let error = repository.statistics(&scope, &range).await.unwrap_err();
        assert_eq!(error.code(), Some(ErrorCode::InvalidParameter), "{error}");
        let error = repository
            .query(&HookExecutionFilter {
                scope,
                range,
                message_id: None,
                success_only: false,
                limit: 10,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some(ErrorCode::InvalidParameter), "{error}");
    }
}
