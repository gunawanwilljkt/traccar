use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use traccar_storage::Storage;

use crate::{ReportError, ReportProvider};

/// Combined report: merges route, events, and summary data.
pub struct CombinedReport {
    storage: Arc<dyn Storage>,
}

impl CombinedReport {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl ReportProvider for CombinedReport {
    fn name(&self) -> &str {
        "combined"
    }

    async fn generate(
        &self,
        device_ids: &[i64],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<serde_json::Value>, ReportError> {
        tracing::debug!("Generating combined report for {} devices", device_ids.len());

        let route = crate::route::RouteReport::new(self.storage.clone());
        let events = crate::events::EventsReport::new(self.storage.clone());
        let summary = crate::summary::SummaryReport::new(self.storage.clone());

        let (positions, event_list, summaries) = tokio::try_join!(
            route.generate(device_ids, from, to),
            events.generate(device_ids, from, to),
            summary.generate(device_ids, from, to),
        )?;

        Ok(vec![serde_json::json!({
            "positions": positions,
            "events": event_list,
            "summary": summaries,
        })])
    }
}
