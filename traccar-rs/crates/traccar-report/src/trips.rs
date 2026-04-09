use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use traccar_storage::Storage;

use crate::{ReportError, ReportProvider};

/// Trips report: detects trips (motion segments) from position data.
#[allow(dead_code)]
pub struct TripsReport {
    storage: Arc<dyn Storage>,
    min_trip_duration_secs: i64,
    min_trip_distance_m: f64,
}

impl TripsReport {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self {
            storage,
            min_trip_duration_secs: 300,
            min_trip_distance_m: 500.0,
        }
    }
}

#[async_trait]
impl ReportProvider for TripsReport {
    fn name(&self) -> &str {
        "trips"
    }

    async fn generate(
        &self,
        device_ids: &[i64],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<serde_json::Value>, ReportError> {
        tracing::debug!("Generating trips report for {} devices", device_ids.len());
        let positions = crate::fetch_positions(self.storage.as_ref(), device_ids, from, to).await?;

        // Parse positions and detect trips based on motion attribute
        let mut trips = Vec::new();
        let mut trip_start: Option<serde_json::Value> = None;
        let mut total_distance = 0.0_f64;

        for pos in &positions {
            let motion = pos
                .get("attributes")
                .and_then(|a| a.get("motion"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if motion && trip_start.is_none() {
                trip_start = Some(pos.clone());
                total_distance = 0.0;
            } else if motion {
                total_distance += pos
                    .get("attributes")
                    .and_then(|a| a.get("distance"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
            } else if let Some(start) = trip_start.take() {
                if total_distance >= self.min_trip_distance_m {
                    trips.push(serde_json::json!({
                        "deviceId": pos.get("deviceId"),
                        "startTime": start.get("fixTime"),
                        "startLat": start.get("latitude"),
                        "startLon": start.get("longitude"),
                        "endTime": pos.get("fixTime"),
                        "endLat": pos.get("latitude"),
                        "endLon": pos.get("longitude"),
                        "distance": total_distance,
                    }));
                }
            }
        }

        Ok(trips)
    }
}
