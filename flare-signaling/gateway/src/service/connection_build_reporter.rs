//! 构建分布上报：每 30s 把本网关按（租户，平台，构建）的在线连接数整份写进 Redis，供 capability 的
//! `ClientBuildStats.GetConnectionBuilds` 汇总（约定见 `flare_im_service_kit::gateway::connection_builds`）。
//!
//! 这一轮没有连接的租户删掉上一轮写的哈希并退出网关集合；停机时删掉全部，不等 TTL。

use std::collections::BTreeSet;
use std::sync::Arc;

use flare_im_service_kit::gateway::connection_builds::{
    REPORT_INTERVAL, hash_fields, write_gateway_builds,
};
use flare_im_service_kit::metrics::AccessGatewayMetrics;
use flare_im_service_kit::redis_connection::{RedisConnectionLimits, SharedRedisConnection};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, warn};

pub struct ConnectionBuildReporter {
    connection: SharedRedisConnection,
    gateway_id: String,
    metrics: Arc<AccessGatewayMetrics>,
    /// 上一轮写过哈希的租户
    written: Mutex<BTreeSet<String>>,
}

impl ConnectionBuildReporter {
    pub fn open(
        redis_url: &str,
        gateway_id: String,
        metrics: Arc<AccessGatewayMetrics>,
    ) -> redis::RedisResult<Self> {
        Ok(Self {
            connection: SharedRedisConnection::new(
                redis::Client::open(redis_url)?,
                RedisConnectionLimits::default(),
            ),
            gateway_id,
            metrics,
            written: Mutex::new(BTreeSet::new()),
        })
    }

    /// 写一轮：有连接的租户整份替换，上一轮有、这一轮没有的租户删掉。
    pub async fn report(&self) -> redis::RedisResult<()> {
        let snapshot = self.metrics.connection_build_snapshot();
        let now_ms = now_ms();
        let mut connection = self.connection.get().await?;
        let mut written = self.written.lock().await;
        let mut current = BTreeSet::new();
        for (tenant_id, platforms) in &snapshot {
            let fields = hash_fields(platforms);
            if fields.is_empty() {
                continue;
            }
            write_gateway_builds(
                &mut connection,
                tenant_id,
                &self.gateway_id,
                &fields,
                now_ms,
            )
            .await?;
            current.insert(tenant_id.clone());
        }
        for tenant_id in written.difference(&current) {
            write_gateway_builds(&mut connection, tenant_id, &self.gateway_id, &[], now_ms).await?;
        }
        debug!(
            gateway_id = %self.gateway_id,
            tenants = current.len(),
            "connection build distribution reported"
        );
        *written = current;
        Ok(())
    }

    /// 删掉本网关写过的全部哈希（停机最后一步）。
    pub async fn clear(&self) -> redis::RedisResult<()> {
        let now_ms = now_ms();
        let mut connection = self.connection.get().await?;
        let mut written = self.written.lock().await;
        for tenant_id in written.iter() {
            write_gateway_builds(&mut connection, tenant_id, &self.gateway_id, &[], now_ms).await?;
        }
        written.clear();
        Ok(())
    }

    /// 上报循环；停机时清掉自己写的数据。
    pub async fn run(
        self: Arc<Self>,
        shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut ticker = tokio::time::interval(REPORT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tokio::pin!(shutdown_rx);
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = ticker.tick() => {
                    if let Err(error) = self.report().await {
                        warn!(gateway_id = %self.gateway_id, %error, "connection build report failed");
                    }
                }
            }
        }
        if let Err(error) = self.clear().await {
            warn!(
                gateway_id = %self.gateway_id,
                %error,
                "connection build report not cleared; it expires on its own"
            );
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
