use crate::{DeviceInfo, Result, TrainerCommand};
use std::future::Future;

/// Interchangeable trainer backend. The future is Send without requiring boxing.
/// Implementations check capabilities and apply safety limits before changing load.
pub trait Trainer: Send + Sync {
    /// Snapshot of device metadata and connection state.
    fn info(&self) -> DeviceInfo;
    /// Establish a connection.
    fn connect(&mut self) -> impl Future<Output = Result<()>> + Send;
    /// Release load and disconnect.
    fn disconnect(&mut self) -> impl Future<Output = Result<()>> + Send;
    /// Apply a command and return the effective, safety-clamped command.
    fn execute(
        &mut self,
        command: TrainerCommand,
    ) -> impl Future<Output = Result<TrainerCommand>> + Send;
    /// Best-effort immediate safe state; must also be usable during teardown.
    fn safe_state(&mut self);
}
