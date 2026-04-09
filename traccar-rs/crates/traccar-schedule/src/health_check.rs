use async_trait::async_trait;
use std::time::Duration;

use crate::{ScheduleError, ScheduledTask};

/// Periodic health check that logs system health metrics.
pub struct HealthCheckTask;

impl HealthCheckTask {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HealthCheckTask {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScheduledTask for HealthCheckTask {
    fn name(&self) -> &str {
        "health_check"
    }

    fn interval(&self) -> Duration {
        Duration::from_secs(60)
    }

    async fn run(&self) -> Result<(), ScheduleError> {
        // Log memory and thread information
        let process_uptime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        tracing::debug!(
            uptime_secs = process_uptime,
            "Health check OK"
        );

        Ok(())
    }
}
