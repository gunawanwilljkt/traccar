pub mod multicast;
pub mod redis;

use async_trait::async_trait;
use traccar_model::{Event, Position};

#[derive(Debug, thiserror::Error)]
pub enum BroadcastError {
    #[error("Broadcast error: {0}")]
    Send(String),
    #[error("IO error: {0}")]
    Io(String),
}

/// Trait for broadcasting position and event updates across server instances.
#[async_trait]
pub trait BroadcastService: Send + Sync {
    /// Start the broadcast service (e.g. bind multicast socket, connect to Redis).
    async fn start(&self) -> Result<(), BroadcastError> {
        Ok(())
    }

    /// Stop the broadcast service.
    async fn stop(&self) -> Result<(), BroadcastError> {
        Ok(())
    }

    /// Broadcast a position update to other instances.
    async fn update_position(&self, position: &Position) -> Result<(), BroadcastError>;

    /// Broadcast an event to other instances.
    async fn update_event(&self, event: &Event) -> Result<(), BroadcastError>;
}

/// No-op broadcast implementation for single-instance deployments.
pub struct NullBroadcast;

impl NullBroadcast {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NullBroadcast {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BroadcastService for NullBroadcast {
    async fn update_position(&self, _position: &Position) -> Result<(), BroadcastError> {
        Ok(())
    }

    async fn update_event(&self, _event: &Event) -> Result<(), BroadcastError> {
        Ok(())
    }
}
