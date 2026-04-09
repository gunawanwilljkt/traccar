use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use traccar_storage::Storage;

use crate::{ReportError, ReportProvider};

/// Summary report: aggregated statistics per device.
pub struct SummaryReport {
    storage: Arc<dyn Storage>,
}

impl SummaryReport {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl ReportProvider for SummaryReport {
    fn name(&self) -> &str {
        "summary"
    }

    async fn generate(
        &self,
        device_ids: &[i64],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<serde_json::Value>, ReportError> {
        tracing::debug!("Generating summary report for {} devices", device_ids.len());

        let mut summaries = Vec::new();
        for &did in device_ids {
            let positions =
                crate::fetch_positions(self.storage.as_ref(), &[did], from, to).await?;

            let mut distance = 0.0_f64;
            let mut max_speed = 0.0_f64;
            let mut engine_hours = 0_i64;

            for pos in &positions {
                distance += pos
                    .get("attributes")
                    .and_then(|a| a.get("distance"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let speed = pos.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if speed > max_speed {
                    max_speed = speed;
                }
                engine_hours += pos
                    .get("attributes")
                    .and_then(|a| a.get("hours"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
            }

            let avg_speed = if !positions.is_empty() {
                let total: f64 = positions
                    .iter()
                    .filter_map(|p| p.get("speed").and_then(|v| v.as_f64()))
                    .sum();
                total / positions.len() as f64
            } else {
                0.0
            };

            summaries.push(serde_json::json!({
                "deviceId": did,
                "distance": distance,
                "maxSpeed": max_speed,
                "averageSpeed": avg_speed,
                "engineHours": engine_hours,
                "positionCount": positions.len(),
            }));
        }

        Ok(summaries)
    }
}
