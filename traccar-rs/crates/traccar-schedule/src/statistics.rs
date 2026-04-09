use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use traccar_storage::Storage;

use crate::{ScheduleError, ScheduledTask};

/// Periodically collects and stores server statistics.
pub struct StatisticsTask {
    storage: Arc<dyn Storage>,
}

impl StatisticsTask {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl ScheduledTask for StatisticsTask {
    fn name(&self) -> &str {
        "statistics"
    }

    fn interval(&self) -> Duration {
        Duration::from_secs(3600) // every hour
    }

    async fn run(&self) -> Result<(), ScheduleError> {
        tracing::info!("Collecting server statistics");

        // Count active devices (those with status = 'online')
        let active_devices = self
            .storage
            .get_objects(
                "tc_devices",
                &traccar_storage::Request::new(traccar_storage::Columns::Include(vec![
                    "id".into(),
                ]))
                .with_condition(traccar_storage::Condition::Equals(
                    "status".into(),
                    serde_json::json!("online"),
                )),
            )
            .await
            .map(|v| v.len() as i32)
            .unwrap_or(0);

        // Count active users
        let active_users = self
            .storage
            .get_objects(
                "tc_users",
                &traccar_storage::Request::new(traccar_storage::Columns::Include(vec![
                    "id".into(),
                ]))
                .with_condition(traccar_storage::Condition::Equals(
                    "disabled".into(),
                    serde_json::json!(false),
                )),
            )
            .await
            .map(|v| v.len() as i32)
            .unwrap_or(0);

        let stats = serde_json::json!({
            "captureTime": chrono::Utc::now().to_rfc3339(),
            "activeDevices": active_devices,
            "activeUsers": active_users,
        });

        self.storage
            .add_object("tc_statistics", &stats, &traccar_storage::Columns::All)
            .await
            .map_err(|e| ScheduleError::Storage(e.to_string()))?;

        tracing::info!(
            active_devices = active_devices,
            active_users = active_users,
            "Statistics recorded"
        );

        Ok(())
    }
}
