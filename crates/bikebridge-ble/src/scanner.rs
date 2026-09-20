use crate::{
    backend::{BackendAdapter, DiscoveryBackend},
    connection::Connections,
    registry::Registry,
};
use bikebridge_core::{
    AdapterInfo, AdapterState, BridgeError, DeviceInfo, ErrorCode, Event, EventBus, NearbyDevice,
    Result, SafetyLimits, ScanStatus, TrainerCommand,
};
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(3);

/// In-memory discovery snapshot; never contains platform-private identities.
#[derive(Debug, Clone, Default)]
pub struct DiscoverySnapshot {
    /// At least one selected adapter is not known to be powered off.
    pub available: bool,
    /// Last enumerated adapters.
    pub adapters: Vec<AdapterInfo>,
    /// Global scan lifecycle.
    pub scan: ScanStatus,
    /// Cycling devices discovered during this daemon session.
    pub devices: Vec<DeviceInfo>,
    /// Cached Bluetooth results, including unnamed and unsupported devices.
    pub nearby: Vec<NearbyDevice>,
}

/// Cloneable discovery actor handle. Commands are serialized independently of API/device locks.
#[derive(Clone)]
pub struct Scanner {
    connections: Connections,
    controllers: bikebridge_openbikecontrol::Controllers,
    snapshot: Arc<RwLock<DiscoverySnapshot>>,
    commands: mpsc::Sender<Request>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}

enum Operation {
    SelectDevice(String),
    SelectName(String),
    Refresh,
    Start,
    Stop,
}
struct Request {
    operation: Operation,
    reply: oneshot::Sender<Result<ScanStatus>>,
}

struct Worker<B> {
    connections: Connections,
    controllers: bikebridge_openbikecontrol::Controllers,
    backend: B,
    snapshot: Arc<RwLock<DiscoverySnapshot>>,
    bus: EventBus,
    registry: Registry,
    nearby: HashMap<(String, String), NearbyDevice>,
    adapter_ids: HashMap<String, String>,
    selected: Option<String>,
    preferred_index: Option<usize>,
}

impl Scanner {
    /// Initialize discovery without starting a scan. Failures are retained in the
    /// snapshot so the daemon still starts without an adapter or OS permission.
    /// `preferred_index` overrides default selection (first powered-on adapter).
    pub async fn spawn(
        backend: impl DiscoveryBackend,
        bus: EventBus,
        preferred_index: Option<usize>,
    ) -> Self {
        Self::spawn_inner(backend, bus, preferred_index, SafetyLimits::default()).await
    }
    /// Initialize discovery and device sessions with validated trainer safety/reconnection settings.
    pub async fn spawn_configured(
        backend: impl DiscoveryBackend,
        bus: EventBus,
        preferred_index: Option<usize>,
        limits: SafetyLimits,
    ) -> Result<Self> {
        limits.validate()?;
        Ok(Self::spawn_inner(backend, bus, preferred_index, limits).await)
    }
    async fn spawn_inner(
        backend: impl DiscoveryBackend,
        bus: EventBus,
        preferred_index: Option<usize>,
        limits: SafetyLimits,
    ) -> Self {
        let snapshot = Arc::new(RwLock::new(DiscoverySnapshot::default()));
        let connections = Connections::with_limits(bus.clone(), limits);
        let controllers = bikebridge_openbikecontrol::Controllers::new(bus.clone());
        let mut worker = Worker {
            connections: connections.clone(),
            controllers: controllers.clone(),
            backend,
            snapshot: snapshot.clone(),
            bus,
            registry: Registry::default(),
            nearby: HashMap::new(),
            adapter_ids: HashMap::new(),
            selected: None,
            preferred_index,
        };
        if let Err(error) = worker.refresh().await {
            worker.fail(error).await;
        }
        let (commands, mut requests) = mpsc::channel::<Request>(8);
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let tasks = TaskTracker::new();
        tasks.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if stop.is_cancelled() {
                    break;
                }
                tokio::select! {
                    _ = stop.cancelled() => break,
                    request = requests.recv() => {
                        let Some(request) = request else { break; };
                        if request.reply.is_closed() { continue; }
                        let result = match request.operation {
                            Operation::SelectDevice(id) => worker.select_device(id).await,
                            Operation::SelectName(name) => worker.select_name(name).await,
                            Operation::Refresh => worker.refresh().await,
                            Operation::Start => worker.start().await,
                            Operation::Stop => worker.stop().await,
                        };
                        if let Err(error) = &result
                            && !matches!(error.code, ErrorCode::DeviceNotFound | ErrorCode::InvalidValue | ErrorCode::UnsupportedOperation)
                        { worker.fail(error.clone()).await; }
                        let status = worker.snapshot.read().await.scan.clone();
                        let _ = request.reply.send(result.map(|()| status));
                    }
                    _ = interval.tick() => worker.poll().await,
                }
            }
            if let Err(error) = worker.stop().await {
                worker.fail(error).await;
            }
        });
        Self {
            connections,
            controllers,
            snapshot,
            commands,
            shutdown,
            tasks,
        }
    }

    /// Read cached metadata without waiting on any platform calls.
    pub async fn snapshot(&self) -> DiscoverySnapshot {
        let mut snapshot = self.snapshot.read().await.clone();
        for device in &mut snapshot.devices {
            self.connections.overlay(device).await;
            self.controllers.overlay(device).await;
        }
        snapshot
    }

    /// Explicitly connect or disconnect a discovered indoor bike. Scanning is independent.
    pub async fn set_connection(&self, id: &str, connected: bool) -> Result<DeviceInfo> {
        self.set_connection_for(0, id, connected).await
    }
    /// Connect/disconnect on behalf of a client; an owned trainer can only be disconnected by its owner.
    pub async fn set_connection_for(
        &self,
        client: usize,
        id: &str,
        connected: bool,
    ) -> Result<DeviceInfo> {
        if !self
            .snapshot
            .read()
            .await
            .devices
            .iter()
            .any(|device| device.id == id)
        {
            return Err(BridgeError::new(
                ErrorCode::DeviceNotFound,
                "Device not found.",
            ));
        }
        if self.controllers.contains(id).await {
            return self.controllers.set_connection(id, connected).await;
        }
        self.connections
            .set_connection_for(client, id, connected)
            .await
    }
    /// Submit a bounded, safety-clamped command to a connected trainer.
    pub async fn execute(
        &self,
        client: usize,
        id: &str,
        command: TrainerCommand,
    ) -> Result<TrainerCommand> {
        if self.controllers.contains(id).await {
            return Err(BridgeError::new(
                ErrorCode::UnsupportedOperation,
                "Controller inputs cannot execute trainer commands.",
            ));
        }
        self.connections.execute(client, id, command).await
    }
    /// Cancel queued commands and release control held by a departing API client.
    pub async fn release_client(&self, client: usize) {
        self.connections.release_client(client).await;
    }

    /// Refresh adapter enumeration while idle; scanning handles are left undisturbed.
    pub async fn refresh(&self) -> Result<ScanStatus> {
        self.request(Operation::Refresh).await
    }
    /// Start scanning, idempotently, on the default/configured adapter.
    pub async fn start(&self) -> Result<ScanStatus> {
        self.request(Operation::Start).await
    }
    /// Include a full Bluetooth name and restart discovery using an unfiltered scan.
    pub async fn select_name(&self, name: String) -> Result<ScanStatus> {
        if name.trim().is_empty()
            || name.chars().count() > 128
            || name.chars().any(char::is_control)
        {
            return Err(BridgeError::new(
                ErrorCode::InvalidValue,
                "Device names must contain 1–128 characters without control characters.",
            ));
        }
        self.request(Operation::SelectName(name)).await
    }
    /// Include one cached nearby peripheral by its opaque selection ID, without connecting.
    pub async fn select_device(&self, id: String) -> Result<ScanStatus> {
        self.request(Operation::SelectDevice(id)).await
    }
    /// Stop scanning, idempotently. Discovered device identities remain cached.
    pub async fn stop(&self) -> Result<ScanStatus> {
        self.request(Operation::Stop).await
    }

    async fn request(&self, operation: Operation) -> Result<ScanStatus> {
        if self.shutdown.is_cancelled() {
            return Err(unavailable());
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(Request { operation, reply })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => BridgeError::new(
                    ErrorCode::Busy,
                    "Discovery command queue is full. Retry later.",
                ),
                mpsc::error::TrySendError::Closed(_) => unavailable(),
            })?;
        tokio::time::timeout(Duration::from_secs(15), response)
            .await
            .map_err(|_| BridgeError::new(ErrorCode::Timeout, "Discovery request timed out."))?
            .map_err(|_| unavailable())?
    }

    /// Cancel and join the actor, attempting to stop the OS scan with a bounded wait.
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        self.connections.shutdown().await;
        self.controllers.shutdown().await;
        self.tasks.close();
        self.tasks.wait().await;
    }
}

fn unavailable() -> BridgeError {
    BridgeError::new(
        ErrorCode::BluetoothUnavailable,
        "Bluetooth discovery is not running.",
    )
}

async fn bounded<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(OPERATION_TIMEOUT, future)
        .await
        .map_err(|_| BridgeError::new(ErrorCode::Timeout, "Bluetooth operation timed out."))?
}

impl<B: DiscoveryBackend> Worker<B> {
    async fn select_device(&mut self, id: String) -> Result<()> {
        let (adapter, key) = self
            .nearby
            .iter()
            .find_map(|(key, value)| {
                (value.id == id && self.selected.as_ref() == Some(&key.0)).then(|| key.clone())
            })
            .ok_or_else(|| {
                BridgeError::new(
                    ErrorCode::DeviceNotFound,
                    "Nearby device not found. Refresh Bluetooth results.",
                )
            })?;
        self.backend.select_device(&adapter, &key)?;
        self.start().await?;
        self.poll().await;
        Ok(())
    }
    async fn select_name(&mut self, name: String) -> Result<()> {
        self.backend.select_name(name)?;
        self.stop().await?;
        self.start().await
    }
    async fn refresh(&mut self) -> Result<()> {
        if self.snapshot.read().await.scan.scanning {
            return Ok(());
        }
        let adapters = match bounded(self.backend.adapters()).await {
            Ok(adapters) => adapters,
            Err(error) => {
                self.selected = None;
                let mut snapshot = self.snapshot.write().await;
                snapshot.adapters.clear();
                snapshot.available = false;
                snapshot.scan.adapter_id = None;
                return Err(error);
            }
        };
        let selected = select_adapter(&adapters, self.preferred_index);
        self.selected = selected.map(|index| adapters[index].key.clone());
        let mut public = Vec::new();
        for (index, adapter) in adapters.iter().enumerate() {
            let id = self
                .adapter_ids
                .entry(adapter.key.clone())
                .or_insert_with(|| {
                    let id = format!("adapter-{}", Uuid::new_v4());
                    tracing::info!(adapter_id = %id, state = ?adapter.state, "BLE adapter detected");
                    id
                })
                .clone();
            public.push(AdapterInfo {
                id,
                name: format!("Bluetooth adapter {}", index + 1),
                state: adapter.state,
                is_default: selected == Some(index),
            });
        }
        let mut snapshot = self.snapshot.write().await;
        snapshot.scan.adapter_id = selected.map(|index| public[index].id.clone());
        snapshot.available =
            selected.is_some_and(|index| adapters[index].state != AdapterState::PoweredOff);
        snapshot.adapters = public;
        if selected.is_none() {
            return Err(BridgeError::new(
                ErrorCode::AdapterNotFound,
                "No matching Bluetooth adapter found.",
            ));
        }
        if !snapshot.available {
            return Err(BridgeError::new(
                ErrorCode::BluetoothUnavailable,
                "Selected Bluetooth radio is powered off.",
            ));
        }
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let current = self.snapshot.read().await.scan.clone();
        if current.scanning && current.last_error.is_none() {
            return Ok(());
        }
        if current.scanning {
            self.stop().await?;
        }
        self.refresh().await?;
        let key = self.selected.clone().ok_or_else(unavailable)?;
        // Mark conservatively before starting: a timed-out OS call may have side effects.
        self.snapshot.write().await.scan.scanning = true;
        if let Err(error) = bounded(self.backend.start_scan(&key)).await {
            if bounded(self.backend.stop_scan(&key)).await.is_ok() {
                self.snapshot.write().await.scan.scanning = false;
            }
            return Err(error);
        }
        self.snapshot.write().await.scan.last_error = None;
        tracing::info!("Bluetooth scan started");
        self.publish_scan().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.snapshot.read().await.scan.scanning {
            return Ok(());
        }
        let key = self.selected.clone().ok_or_else(unavailable)?;
        bounded(self.backend.stop_scan(&key)).await?;
        {
            let mut snapshot = self.snapshot.write().await;
            snapshot.scan.scanning = false;
            snapshot.scan.last_error = None;
        }
        tracing::info!("Bluetooth scan stopped");
        self.publish_scan().await;
        Ok(())
    }

    async fn poll(&mut self) {
        let current = self.snapshot.read().await.scan.clone();
        if !current.scanning || current.last_error.is_some() {
            return;
        }
        let Some(key) = &self.selected else {
            return;
        };
        match bounded(self.backend.advertisements(key)).await {
            Ok(advertisements) => {
                let mut events = Vec::new();
                for advertisement in advertisements {
                    let nearby_key = (key.clone(), advertisement.key.clone());
                    if self.nearby.len() < 1024 || self.nearby.contains_key(&nearby_key) {
                        let result =
                            self.nearby
                                .entry(nearby_key)
                                .or_insert_with(|| NearbyDevice {
                                    id: format!("nearby-{}", Uuid::new_v4()),
                                    name: None,
                                    signal_strength: None,
                                    device_id: None,
                                });
                        if let Some(name) = advertisement
                            .name
                            .as_ref()
                            .filter(|name| !name.trim().is_empty())
                        {
                            result.name =
                                Some(name.chars().filter(|c| !c.is_control()).take(128).collect());
                        }
                        if advertisement.rssi.is_some() {
                            result.signal_strength = advertisement.rssi;
                        }
                    }
                    if let Some(event) = self.registry.observe(key, advertisement) {
                        events.push(event);
                    }
                }
                let mut nearby = Vec::new();
                for ((adapter, peripheral), result) in &mut self.nearby {
                    if adapter == key {
                        result.device_id = self.registry.device_id(adapter, peripheral);
                        nearby.push(result.clone());
                    }
                }
                nearby.sort_by(|a, b| {
                    a.name
                        .is_none()
                        .cmp(&b.name.is_none())
                        .then_with(|| {
                            a.name
                                .as_deref()
                                .unwrap_or("")
                                .to_lowercase()
                                .cmp(&b.name.as_deref().unwrap_or("").to_lowercase())
                        })
                        .then_with(|| a.id.cmp(&b.id))
                });
                self.snapshot.write().await.nearby = nearby;
                if !events.is_empty() {
                    for event in &mut events {
                        if let Event::DeviceDiscovered { data } | Event::DeviceUpdated { data } =
                            event
                            && data.kind == bikebridge_core::DeviceKind::BikeController
                            && let Some((adapter, key)) = self.registry.private_keys(&data.id)
                            && let Some(transport) = self.backend.controller(adapter, key)
                        {
                            self.controllers.register(data, transport).await;
                        }
                        if let Event::DeviceDiscovered { data } | Event::DeviceUpdated { data } =
                            event
                            && matches!(
                                data.kind,
                                bikebridge_core::DeviceKind::Trainer
                                    | bikebridge_core::DeviceKind::PowerMeter
                                    | bikebridge_core::DeviceKind::Unknown
                            )
                            && let Some((adapter, key)) = self.registry.private_keys(&data.id)
                            && let Some(transport) = self.backend.peripheral(adapter, key)
                        {
                            self.connections.register(data, transport).await;
                        }
                    }
                    self.snapshot.write().await.devices = self.registry.devices();
                    for event in events {
                        if let Event::DeviceDiscovered { data } = &event {
                            tracing::info!(device_id = %data.id, name = %data.name, kind = ?data.kind, "Cycling device candidate discovered");
                        }
                        if matches!(&event,Event::DeviceDiscovered {data} | Event::DeviceUpdated {data} if data.kind==bikebridge_core::DeviceKind::BikeController)
                        {
                            self.controllers.publish_discovery(event).await;
                        } else {
                            self.connections.publish_discovery(event).await;
                        }
                    }
                }
            }
            Err(error) => {
                let _ = self.stop().await;
                if error.code == ErrorCode::BluetoothUnavailable {
                    let mut snapshot = self.snapshot.write().await;
                    snapshot.available = false;
                    for adapter in &mut snapshot.adapters {
                        if adapter.is_default {
                            adapter.state = AdapterState::Unknown;
                        }
                    }
                }
                self.fail(error).await;
            }
        }
    }

    async fn fail(&mut self, error: BridgeError) {
        let changed = self.snapshot.read().await.scan.last_error.as_ref() != Some(&error);
        self.snapshot.write().await.scan.last_error = Some(error.clone());
        if changed {
            tracing::warn!(code = ?error.code, message = %error.message, "Bluetooth discovery failed");
            self.bus.publish(Event::Error { data: error });
            self.publish_scan().await;
        }
    }

    async fn publish_scan(&mut self) {
        self.bus.publish(Event::ScanStatus {
            data: self.snapshot.read().await.scan.clone(),
        });
    }
}

fn select_adapter(adapters: &[BackendAdapter], preferred: Option<usize>) -> Option<usize> {
    if let Some(index) = preferred {
        return (index < adapters.len()).then_some(index);
    }
    adapters
        .iter()
        .position(|adapter| adapter.state == AdapterState::PoweredOn)
        .or_else(|| {
            adapters
                .iter()
                .position(|adapter| adapter.state == AdapterState::Unknown)
        })
        .or_else(|| (!adapters.is_empty()).then_some(0))
}
