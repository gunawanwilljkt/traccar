use async_trait::async_trait;
use traccar_model::{Event, Position};

use crate::{BroadcastError, BroadcastService};

/// Broadcasts updates via UDP multicast for multi-instance synchronization.
pub struct MulticastBroadcast {
    pub multicast_group: String,
    pub port: u16,
}

impl MulticastBroadcast {
    pub fn new(group: &str, port: u16) -> Self {
        Self {
            multicast_group: group.to_string(),
            port,
        }
    }
}

impl Default for MulticastBroadcast {
    fn default() -> Self {
        Self::new("224.0.0.114", 9443)
    }
}

#[async_trait]
impl BroadcastService for MulticastBroadcast {
    async fn start(&self) -> Result<(), BroadcastError> {
        tracing::info!(
            group = %self.multicast_group,
            port = self.port,
            "Starting multicast broadcast"
        );
        Ok(())
    }

    async fn update_position(&self, position: &Position) -> Result<(), BroadcastError> {
        let payload = serde_json::to_vec(position)
            .map_err(|e| BroadcastError::Send(e.to_string()))?;

        tracing::debug!(
            device_id = position.device_id,
            bytes = payload.len(),
            "Multicast position update (stub)"
        );

        // A full implementation would send `payload` to the multicast group.
        Ok(())
    }

    async fn update_event(&self, event: &Event) -> Result<(), BroadcastError> {
        let payload = serde_json::to_vec(event)
            .map_err(|e| BroadcastError::Send(e.to_string()))?;

        tracing::debug!(
            device_id = event.device_id,
            event_type = %event.event_type,
            bytes = payload.len(),
            "Multicast event update (stub)"
        );

        Ok(())
    }
}
