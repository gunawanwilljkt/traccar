use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use traccar_storage::Storage;

use crate::{ScheduleError, ScheduledTask};

/// Checks for devices that have not reported in a configurable period
/// and generates deviceInactive events.
pub struct DeviceInactiveTask {
    storage: Arc<dyn Storage>,
    inactive_threshold: Duration,
}

impl DeviceInactiveTask {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            storage,
            inactive_threshold: Duration::from_secs(3600), // 1 hour default
        }
    }

    pub fn with_threshold(mut self, threshold: Duration) -> Self {
        self.inactive_threshold = threshold;
        self
    }
}

#[async_trait]
impl ScheduledTask for DeviceInactiveTask {
    fn name(&self) -> &str {
        "device_inactive"
    }

    fn interval(&self) -> Duration {
        Duration::from_secs(300) // every 5 minutes
    }

    async fn run(&self) -> Result<(), ScheduleError> {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(self.inactive_threshold)
            .unwrap_or(chrono::Duration::hours(1));

        // Find devices whose lastUpdate is before the cutoff and status is online
        let request = traccar_storage::Request::new(traccar_storage::Columns::Include(vec![
            "id".into(),
            "name".into(),
            "lastUpdate".into(),
        ]))
        .with_condition(traccar_storage::Condition::And(
            Box::new(traccar_storage::Condition::Equals(
                "status".into(),
                serde_json::json!("online"),
            )),
            Box::new(traccar_storage::Condition::LessThan(
                "lastUpdate".into(),
                serde_json::json!(cutoff.to_rfc3339()),
            )),
        ));

        let devices = self
            .storage
            .get_objects("tc_devices", &request)
            .await
            .unwrap_or_default();

        if !devices.is_empty() {
            tracing::info!("{} devices detected as inactive", devices.len());
        }

        for device in &devices {
            let device_id = device.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let event = serde_json::json!({
                "deviceId": device_id,
                "type": "deviceInactive",
                "eventTime": chrono::Utc::now().to_rfc3339(),
            });
            let _ = self
                .storage
                .add_object("tc_events", &event, &traccar_storage::Columns::All)
                .await;
        }

        Ok(())
    }
}
