//! Explicit application state and mock-device orchestration.
use crate::protocol::{Command, Subscription};
use bikebridge_ble::Scanner;
use bikebridge_core::*;
use bikebridge_mock::{CONTROLLER_ID, MockController, MockTrainer, TRAINER_ID};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Daemon state, cheaply cloned into HTTP and WebSocket handlers.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
    /// Bounded event stream.
    pub events: EventBus,
    pub(crate) shutdown: CancellationToken,
    pub(crate) sessions: TaskTracker,
    scanner: Option<Scanner>,
    replay: Option<Arc<Mutex<crate::replay::ReplayRuntime>>>,
}

struct Inner {
    started: Instant,
    mock: bool,
    clients: AtomicUsize,
    next_session: AtomicUsize,
    next_command: AtomicU64,
    registry: Mutex<Registry>,
}

struct Registry {
    trainer: Option<MockTrainer>,
    controller: Option<MockController>,
    owner: Option<usize>,
}

/// Health and capability snapshot, including the independent discovery backend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Daemon version.
    pub version: &'static str,
    /// Wire protocol version.
    pub protocol_version: u8,
    /// Monotonic process uptime.
    pub uptime_seconds: u64,
    /// Whether a real Bluetooth backend is available.
    pub bluetooth_available: bool,
    /// Whether the daemon has a real discovery backend (false in isolated mock mode).
    pub bluetooth_enabled: bool,
    /// Scan lifecycle and latest discovery failure.
    pub scan: ScanStatus,
    /// Whether mock devices are enabled.
    pub mock_mode: bool,
    /// Present only in hardware-free replay mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<bikebridge_trace::PlaybackStatus>,
    /// Number of open WebSocket sessions.
    pub websocket_clients: usize,
    /// Number of connected devices.
    pub connected_devices: usize,
}

impl AppState {
    /// Create isolated daemon state. No global mutable state or background task is created.
    pub fn new(mock: bool, limits: SafetyLimits) -> Result<Self> {
        limits.validate()?;
        Ok(Self {
            inner: Arc::new(Inner {
                started: Instant::now(),
                mock,
                clients: AtomicUsize::new(0),
                next_session: AtomicUsize::new(1),
                next_command: AtomicU64::new(1),
                registry: Mutex::new(Registry {
                    trainer: if mock {
                        Some(MockTrainer::new(limits)?)
                    } else {
                        None
                    },
                    controller: mock.then(MockController::default),
                    owner: None,
                }),
            }),
            events: EventBus::default(),
            shutdown: CancellationToken::new(),
            sessions: TaskTracker::new(),
            scanner: None,
            replay: None,
        })
    }

    /// Create an isolated, paused replay daemon. This constructs no Bluetooth backend.
    pub fn replay(trace: bikebridge_trace::Trace, speed: f64) -> anyhow::Result<Self> {
        let mut state = Self::new(false, SafetyLimits::default())?;
        state.replay = Some(Arc::new(Mutex::new(crate::replay::ReplayRuntime::new(
            trace, speed,
        )?)));
        Ok(state)
    }

    /// Timeline state, absent during live operation.
    pub async fn replay_status(&self) -> Option<bikebridge_trace::PlaybackStatus> {
        match &self.replay {
            Some(runtime) => Some(runtime.lock().await.status()),
            None => None,
        }
    }

    /// Start, pause, or restart the virtual timeline; never touches live devices.
    pub async fn replay_action(&self, action: &str) -> Result<bikebridge_trace::PlaybackStatus> {
        let runtime = self.replay.as_ref().ok_or_else(|| {
            BridgeError::new(
                ErrorCode::UnsupportedOperation,
                "This daemon is not replaying a trace.",
            )
        })?;
        runtime.lock().await.action(action, &self.events)
    }

    pub(crate) async fn replay_tick(&self) {
        if let Some(runtime) = &self.replay {
            runtime.lock().await.tick(&self.events);
        }
    }

    /// Attach discovery initialized with this state's event bus.
    pub fn with_scanner(mut self, scanner: Scanner) -> Self {
        self.scanner = Some(scanner);
        self
    }

    /// Snapshot of available adapters, refreshing native enumeration when idle.
    pub async fn adapters(&self) -> Vec<AdapterInfo> {
        if let Some(scanner) = &self.scanner {
            let _ = scanner.refresh().await; // Error remains visible in status.scan.lastError.
            scanner.snapshot().await.adapters
        } else {
            Vec::new()
        }
    }

    /// Start or stop the process-wide scan without holding a device registry lock.
    pub async fn scan(&self, start: bool) -> Result<ScanStatus> {
        let scanner = self.scanner.as_ref().ok_or_else(|| {
            BridgeError::new(
                ErrorCode::BluetoothUnavailable,
                "Bluetooth discovery is disabled in mock mode.",
            )
        })?;
        if start {
            scanner.start().await
        } else {
            scanner.stop().await
        }
    }

    /// Cached Bluetooth results, including devices whose cycling support is unknown.
    pub async fn nearby_devices(&self) -> Vec<bikebridge_core::NearbyDevice> {
        match &self.scanner {
            Some(scanner) => scanner.snapshot().await.nearby,
            None => Vec::new(),
        }
    }

    /// Select exactly one peripheral from the nearby scan results.
    pub async fn select_nearby_device(&self, id: String) -> Result<ScanStatus> {
        let scanner = self.scanner.as_ref().ok_or_else(|| {
            BridgeError::new(
                ErrorCode::BluetoothUnavailable,
                "Bluetooth discovery is not enabled in this session.",
            )
        })?;
        scanner.select_device(id).await
    }

    /// Include a full Bluetooth name in discovery for this daemon session.
    pub async fn select_device_name(&self, name: String) -> Result<ScanStatus> {
        let scanner = self.scanner.as_ref().ok_or_else(|| {
            BridgeError::new(
                ErrorCode::BluetoothUnavailable,
                "Bluetooth discovery is not enabled in this session.",
            )
        })?;
        scanner.select_name(name).await
    }

    /// Stop and join the discovery actor during daemon shutdown.
    pub async fn shutdown_discovery(&self) {
        if let Some(scanner) = &self.scanner {
            scanner.shutdown().await;
        }
    }

    async fn ble_device(&self, id: &str) -> bool {
        if let Some(scanner) = &self.scanner {
            scanner
                .snapshot()
                .await
                .devices
                .iter()
                .any(|device| device.id == id)
        } else {
            false
        }
    }

    /// Snapshot of all known devices.
    pub async fn devices(&self) -> Vec<DeviceInfo> {
        if let Some(runtime) = &self.replay {
            return runtime.lock().await.devices();
        }
        let registry = self.inner.registry.lock().await;
        let mut devices: Vec<_> = registry
            .trainer
            .iter()
            .map(Trainer::info)
            .chain(registry.controller.iter().map(MockController::info))
            .collect();
        drop(registry);
        if let Some(scanner) = &self.scanner {
            devices.extend(scanner.snapshot().await.devices);
        }
        devices
    }

    /// Status without private transport identifiers.
    pub async fn status(&self) -> Status {
        let discovery = match &self.scanner {
            Some(scanner) => scanner.snapshot().await,
            None => Default::default(),
        };
        Status {
            version: env!("CARGO_PKG_VERSION"),
            protocol_version: 1,
            uptime_seconds: self.inner.started.elapsed().as_secs(),
            bluetooth_available: discovery.available,
            bluetooth_enabled: self.scanner.is_some(),
            scan: discovery.scan,
            mock_mode: self.inner.mock,
            replay: self.replay_status().await,
            websocket_clients: self.inner.clients.load(Ordering::Relaxed),
            connected_devices: self.devices().await.iter().filter(|d| d.connected).count(),
        }
    }

    pub(crate) fn open_session(&self) -> usize {
        self.inner.clients.fetch_add(1, Ordering::Relaxed);
        self.inner.next_session.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) async fn close_session(&self, session: usize) {
        if self.replay.is_none() {
            self.events.publish(Event::SessionDisconnected {
                session_id: session as u64,
            });
        }
        let mut registry = self.inner.registry.lock().await;
        if registry.owner == Some(session) {
            if let Some(trainer) = &mut registry.trainer {
                trainer.safe_state();
            }
            registry.owner = None;
            tracing::info!(
                session,
                "Trainer reset after controlling client disconnected"
            );
        }
        self.inner.clients.fetch_sub(1, Ordering::Relaxed);
        drop(registry);
        if let Some(scanner) = &self.scanner {
            scanner.release_client(session).await;
        }
    }

    /// Reset all loads before daemon teardown, including when the listener fails.
    pub async fn safe_state(&self) {
        let mut registry = self.inner.registry.lock().await;
        if let Some(trainer) = &mut registry.trainer {
            trainer.safe_state();
        }
        registry.owner = None;
    }

    pub(crate) async fn tick(&self, elapsed: Duration) {
        let sample = self
            .inner
            .registry
            .lock()
            .await
            .trainer
            .as_mut()
            .and_then(|t| t.tick(elapsed));
        if let Some(data) = sample {
            self.events.publish(Event::Telemetry {
                device_id: TRAINER_ID.into(),
                data,
            });
        }
    }

    pub(crate) async fn execute(
        &self,
        session: usize,
        command: Command,
        subscription: &mut Subscription,
    ) -> Result<Value> {
        if let Command::Subscribe { events, device_ids } = command {
            if events.len() > 8
                || device_ids.len() > 64
                || device_ids.iter().any(|id| id.is_empty() || id.len() > 128)
            {
                return Err(BridgeError::new(
                    ErrorCode::InvalidValue,
                    "Subscription exceeds allowed size.",
                ));
            }
            *subscription = Subscription { events, device_ids };
            return Ok(json!({}));
        }
        if let Some((id, operation)) = command.trainer_command() {
            if self.replay.is_some() {
                return Err(BridgeError::new(
                    ErrorCode::UnsupportedOperation,
                    "Replay is passive: recorded commands are events, and live load commands are not executed.",
                ));
            }
            let mut audit = CommandAudit::start(
                self.events.clone(),
                self.inner.next_command.fetch_add(1, Ordering::Relaxed),
                session,
                id,
                operation,
            );
            let result = self.execute_trainer(session, id, operation).await;
            audit.finish(&result);
            return result.map(|applied| json!({"applied":applied}));
        }
        let mut registry = self.inner.registry.lock().await;
        match command {
            Command::MockTelemetry { device_id, data } => {
                require_trainer(&registry, &device_id)?;
                require_owner(&registry, session)?;
                registry
                    .trainer
                    .as_mut()
                    .ok_or_else(not_found)?
                    .set_telemetry(data)?;
                registry.owner = Some(session);
                Ok(json!({}))
            }
            Command::MockInput { device_id, data } => {
                if device_id != CONTROLLER_ID {
                    return Err(if device_id == TRAINER_ID && registry.trainer.is_some() {
                        BridgeError::new(
                            ErrorCode::UnsupportedOperation,
                            "This device is not a controller.",
                        )
                    } else {
                        not_found()
                    });
                }
                let event = registry
                    .controller
                    .as_ref()
                    .ok_or_else(not_found)?
                    .input(data)?;
                self.events.publish(event);
                Ok(json!({}))
            }
            _ => Err(BridgeError::new(
                ErrorCode::UnsupportedOperation,
                "Unsupported operation.",
            )),
        }
    }

    async fn execute_trainer(
        &self,
        session: usize,
        id: &str,
        operation: TrainerCommand,
    ) -> Result<TrainerCommand> {
        if self.ble_device(id).await {
            return self
                .scanner
                .as_ref()
                .ok_or_else(not_found)?
                .execute(session, id, operation)
                .await;
        }
        let mut registry = self.inner.registry.lock().await;
        require_trainer(&registry, id)?;
        require_owner(&registry, session)?;
        let trainer = registry.trainer.as_mut().ok_or_else(not_found)?;
        let applied = match trainer.execute(operation).await {
            Ok(applied) => applied,
            Err(error) => {
                trainer.safe_state();
                registry.owner = None;
                tracing::warn!(session, code = ?error.code, "Trainer command failed; reset to safe state");
                return Err(error);
            }
        };
        registry.owner = if matches!(operation, TrainerCommand::Reset) {
            None
        } else {
            Some(session)
        };
        tracing::debug!(session, ?applied, "Trainer command completed");
        Ok(applied)
    }

    pub(crate) async fn set_connection(
        &self,
        session: usize,
        id: &str,
        connected: bool,
    ) -> Result<Value> {
        if let Some(runtime) = &self.replay {
            let device = runtime
                .lock()
                .await
                .devices()
                .into_iter()
                .find(|d| d.id == id)
                .ok_or_else(not_found)?;
            if device.connected == connected {
                return Ok(json!(device));
            }
            return Err(BridgeError::new(
                ErrorCode::UnsupportedOperation,
                "Replay connection state follows the trace timeline.",
            ));
        }
        if self.ble_device(id).await {
            return self
                .scanner
                .as_ref()
                .ok_or_else(not_found)?
                .set_connection_for(session, id, connected)
                .await
                .map(|data| json!(data));
        }
        let mut registry = self.inner.registry.lock().await;
        let (before, after) = match id {
            TRAINER_ID => {
                require_owner(&registry, session)?;
                let trainer = registry.trainer.as_mut().ok_or_else(not_found)?;
                let before = trainer.info();
                if before.connected != connected {
                    if connected {
                        trainer.connect().await?;
                    } else {
                        trainer.disconnect().await?;
                    }
                }
                let after = trainer.info();
                if !connected {
                    registry.owner = None;
                }
                (before, after)
            }
            CONTROLLER_ID => {
                let controller = registry.controller.as_mut().ok_or_else(not_found)?;
                let before = controller.info();
                controller.set_connected(connected);
                (before, controller.info())
            }
            _ => return Err(not_found()),
        };
        if before.connected != after.connected {
            tracing::info!(device_id = id, connected, "Device connection changed");
            self.events.publish(if connected {
                Event::DeviceConnected {
                    data: after.clone(),
                }
            } else {
                Event::DeviceDisconnected {
                    data: after.clone(),
                }
            });
        }
        Ok(json!(after))
    }
}

fn require_trainer(registry: &Registry, id: &str) -> Result<()> {
    if id == TRAINER_ID && registry.trainer.is_some() {
        return Ok(());
    }
    if id == CONTROLLER_ID && registry.controller.is_some() {
        return Err(BridgeError::new(
            ErrorCode::UnsupportedOperation,
            "This device is not a trainer.",
        ));
    }
    Err(not_found())
}
fn require_owner(registry: &Registry, session: usize) -> Result<()> {
    if registry.owner.is_some_and(|owner| owner != session) {
        Err(BridgeError::new(
            ErrorCode::TrainerControlDenied,
            "Another client owns trainer control.",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn not_found() -> BridgeError {
    BridgeError::new(ErrorCode::DeviceNotFound, "Device not found.")
}

// Drop covers futures cancelled by socket closure or shutdown, preserving uncertain outcomes.
struct CommandAudit {
    events: EventBus,
    id: u64,
    device: String,
    finished: bool,
}
impl CommandAudit {
    fn start(
        events: EventBus,
        id: u64,
        session: usize,
        device: &str,
        command: TrainerCommand,
    ) -> Self {
        events.publish(Event::CommandStarted {
            command_id: id,
            session_id: session as u64,
            device_id: device.into(),
            command,
        });
        Self {
            events,
            id,
            device: device.into(),
            finished: false,
        }
    }
    fn finish(&mut self, result: &Result<TrainerCommand>) {
        let outcome = match result {
            Ok(command) => CommandOutcome::Applied { command: *command },
            Err(error) => CommandOutcome::Failed {
                error: error.clone(),
            },
        };
        self.events.publish(Event::CommandFinished {
            command_id: self.id,
            device_id: self.device.clone(),
            outcome,
        });
        self.finished = true;
    }
}
impl Drop for CommandAudit {
    fn drop(&mut self) {
        if !self.finished {
            self.events.publish(Event::CommandFinished {
                command_id: self.id,
                device_id: self.device.clone(),
                outcome: CommandOutcome::Cancelled,
            });
        }
    }
}
