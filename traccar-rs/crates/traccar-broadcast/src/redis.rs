use async_trait::async_trait;
use traccar_model::{Event, Position};

use crate::{BroadcastError, BroadcastService};

/// Broadcasts updates via Redis Pub/Sub for multi-instance synchronization.
pub struct RedisBroadcast {
    pub url: String,
    pub channel: String,
}

impl RedisBroadcast {
    pub fn new(url: &str, channel: &str) -> Self {
        Self {
            url: url.to_string(),
            channel: channel.to_string(),
        }
    }
}

#[async_trait]
impl BroadcastService for RedisBroadcast {
    async fn start(&self) -> Result<(), BroadcastError> {
        tracing::info!(
            url = %self.url,
            channel = %self.channel,
            "Starting Redis broadcast"
        );
        Ok(())
    }

    async fn update_position(&self, position: &Position) -> Result<(), BroadcastError> {
        let payload = serde_json::to_string(&serde_json::json!({
            "type": "position",
            "data": position,
        }))
        .map_err(|e| BroadcastError::Send(e.to_string()))?;

        tracing::debug!(
            channel = %self.channel,
            device_id = position.device_id,
            "Redis PUBLISH position (stub)"
        );

        // A full implementation would use the redis crate to PUBLISH.
        let _ = payload;
        Ok(())
    }

    async fn update_event(&self, event: &Event) -> Result<(), BroadcastError> {
        let payload = serde_json::to_string(&serde_json::json!({
            "type": "event",
            "data": event,
        }))
        .map_err(|e| BroadcastError::Send(e.to_string()))?;

        tracing::debug!(
            channel = %self.channel,
            device_id = event.device_id,
            event_type = %event.event_type,
            "Redis PUBLISH event (stub)"
        );

        let _ = payload;
        Ok(())
    }
}
