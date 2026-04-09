use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use traccar_storage::Storage;

use crate::{ReportError, ReportProvider};

/// Stops report: detects periods where the device was stationary.
#[allow(dead_code)]
pub struct StopsReport {
    storage: Arc<dyn Storage>,
    min_stop_duration_secs: i64,
}

impl StopsReport {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            storage,
            min_stop_duration_secs: 300,
        }
    }
}

#[async_trait]
impl ReportProvider for StopsReport {
    fn name(&self) -> &str {
        "stops"
    }

    async fn generate(
        &self,
        device_ids: &[i64],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<serde_json::Value>, ReportError> {
        tracing::debug!("Generating stops report for {} devices", device_ids.len());
        let positions = crate::fetch_positions(self.storage.as_ref(), device_ids, from, to).await?;

        let mut stops = Vec::new();
        let mut stop_start: Option<&serde_json::Value> = None;

        for pos in &positions {
            let motion = pos
                .get("attributes")
                .and_then(|a| a.get("motion"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !motion && stop_start.is_none() {
                stop_start = Some(pos);
            } else if motion {
                if let Some(start) = stop_start.take() {
                    stops.push(serde_json::json!({
                        "deviceId": start.get("deviceId"),
                        "startTime": start.get("fixTime"),
                        "endTime": pos.get("fixTime"),
                        "latitude": start.get("latitude"),
                        "longitude": start.get("longitude"),
                        "address": start.get("address"),
                    }));
                }
            }
        }

        // Close any open stop at the end of the range
        if let Some(start) = stop_start {
            stops.push(serde_json::json!({
                "deviceId": start.get("deviceId"),
                "startTime": start.get("fixTime"),
                "endTime": to.to_rfc3339(),
                "latitude": start.get("latitude"),
                "longitude": start.get("longitude"),
                "address": start.get("address"),
            }));
        }

        Ok(stops)
    }
}
