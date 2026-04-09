pub mod statistics;
pub mod device_inactive;
pub mod health_check;

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use traccar_storage::Storage;

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("Task error: {0}")]
    Task(String),
    #[error("Storage error: {0}")]
    Storage(String),
}

impl From<traccar_storage::StorageError> for ScheduleError {
    fn from(e: traccar_storage::StorageError) -> Self {
        ScheduleError::Storage(e.to_string())
    }
}

/// Trait for periodic background tasks.
#[async_trait]
pub trait ScheduledTask: Send + Sync {
    /// Task name for logging.
    fn name(&self) -> &str;

    /// How often the task should run.
    fn interval(&self) -> Duration;

    /// Execute the task.
    async fn run(&self) -> Result<(), ScheduleError>;
}

/// Schedule manager that runs tasks on their configured intervals.
pub struct ScheduleManager {
    tasks: Vec<Box<dyn ScheduledTask>>,
}

impl ScheduleManager {
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    pub fn add_task(&mut self, task: Box<dyn ScheduledTask>) {
        self.tasks.push(task);
    }

    /// Start all tasks as background tokio tasks.
    pub fn start(self) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();
        for task in self.tasks {
            let name = task.name().to_string();
            let interval = task.interval();
            handles.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    tracing::debug!(task = %name, "Running scheduled task");
                    if let Err(e) = task.run().await {
                        tracing::error!(task = %name, "Scheduled task failed: {}", e);
                    }
                }
            }));
        }
        handles
    }
}

impl Default for ScheduleManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function to set up and run the default tasks.
pub async fn run_scheduler(_config: &(dyn std::any::Any + Send + Sync), storage: Arc<dyn Storage>) {
    tracing::info!("Scheduler started");

    let mut manager = ScheduleManager::new();
    manager.add_task(Box::new(statistics::StatisticsTask::new(storage.clone())));
    manager.add_task(Box::new(device_inactive::DeviceInactiveTask::new(storage.clone())));
    manager.add_task(Box::new(health_check::HealthCheckTask::new()));

    let _handles = manager.start();

    // Wait forever (tasks run in background)
    futures::future::pending::<()>().await;
}
