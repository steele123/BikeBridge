use super::*;
#[path = "control_tests.rs"]
mod control_tests;
use crate::transport::FtmsSession;
use crate::{
    control::ControlProfile,
    transport::{ControlEvent, ControlSession},
};
use bikebridge_core::{DeviceCapability, DeviceKind};
use futures_util::{future::BoxFuture, stream};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};

#[derive(Default)]
struct Fake {
    cycling_power: AtomicBool,
    control_enabled: AtomicBool,
    reply_mode: AtomicUsize,
    emit_status: AtomicBool,
    writes: Mutex<Vec<(tokio::time::Instant, Vec<u8>)>>,
    control_sender: Mutex<Option<mpsc::Sender<ControlEvent>>>,
    opens: AtomicUsize,
    closes: AtomicUsize,
    open_mode: AtomicUsize,
    fail_close: AtomicBool,
    connected: AtomicBool,
    sender: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
}
impl FtmsTransport for Fake {
    fn write_control<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.writes
                .lock()
                .expect("mutex")
                .push((tokio::time::Instant::now(), bytes.to_vec()));
            let mode = self.reply_mode.load(SeqCst);
            if mode == 5 {
                return Err(BridgeError::new(
                    ErrorCode::ConnectionFailed,
                    "Write failed.",
                ));
            }
            if mode != 1 {
                let sender = self
                    .control_sender
                    .lock()
                    .expect("mutex")
                    .as_ref()
                    .expect("control stream")
                    .clone();
                let response = vec![
                    128,
                    if mode == 2 {
                        bytes[0].wrapping_add(1)
                    } else {
                        bytes[0]
                    },
                    match mode {
                        3 => 5,
                        4 => 4,
                        _ => 1,
                    },
                ];
                sender
                    .send(ControlEvent::Indication(response))
                    .await
                    .map_err(|_| crate::session::disconnected())?;
                if self.emit_status.load(SeqCst) {
                    let status = match bytes[0] {
                        8 => Some(vec![2, 1]),
                        1 => Some(vec![1]),
                        _ => None,
                    };
                    if let Some(status) = status {
                        sender
                            .send(ControlEvent::Status(status))
                            .await
                            .map_err(|_| crate::session::disconnected())?;
                    }
                }
            }
            Ok(())
        })
    }
    fn open(&self) -> BoxFuture<'_, Result<FtmsSession>> {
        Box::pin(async move {
            self.opens.fetch_add(1, SeqCst);
            self.connected.store(true, SeqCst); // A failure may occur after the OS connected.
            match self.open_mode.load(SeqCst) {
                1 => {
                    return Err(BridgeError::new(
                        ErrorCode::UnsupportedOperation,
                        "Missing Indoor Bike Data.",
                    ));
                }
                2 => std::future::pending::<()>().await,
                _ => {}
            }
            let (sender, receiver) = mpsc::channel(16);
            *self.sender.lock().expect("mutex") = Some(sender);
            let control = if self.control_enabled.load(SeqCst) {
                let (sender, receiver) = mpsc::channel(32);
                *self.control_sender.lock().expect("mutex") = Some(sender);
                Some(ControlSession {
                    profile: ControlProfile::discover(
                        &[0, 0, 0, 0, 12, 32, 0, 0],
                        Some(&[0, 0, 232, 3, 10, 0]),
                        Some(&[0, 0, 220, 5, 5, 0]),
                    )
                    .expect("profile"),
                    events: Box::pin(stream::unfold(receiver, |mut receiver| async move {
                        receiver.recv().await.map(|event| (event, receiver))
                    })),
                })
            } else {
                None
            };
            Ok(FtmsSession {
                format: if self.cycling_power.load(SeqCst) {
                    TelemetryFormat::CyclingPower
                } else {
                    TelemetryFormat::Ftms
                },
                control,
                features: if self.cycling_power.load(SeqCst) {
                    vec![8, 0, 0, 0]
                } else if self.open_mode.load(SeqCst) == 3 {
                    vec![0]
                } else {
                    vec![2, 0x44, 0, 0, 255, 255, 255, 255]
                },
                notifications: Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver.recv().await.map(|bytes| (bytes, receiver))
                })),
            })
        })
    }
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(self.connected.load(SeqCst)) })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async {
            self.closes.fetch_add(1, SeqCst);
            if self.fail_close.load(SeqCst) {
                return Err(BridgeError::new(
                    ErrorCode::ConnectionFailed,
                    "Fixture disconnect failure.",
                ));
            }
            self.connected.store(false, SeqCst);
            self.sender.lock().expect("mutex").take();
            self.control_sender.lock().expect("mutex").take();
            Ok(())
        })
    }
}
fn device() -> DeviceInfo {
    DeviceInfo {
        id: "ble-fixture".into(),
        name: "Trainer".into(),
        kind: DeviceKind::Trainer,
        transport: "bluetooth".into(),
        connected: false,
        signal_strength: Some(-50),
        capabilities: vec![],
    }
}
async fn fixture() -> (
    Connections,
    Arc<Fake>,
    tokio::sync::broadcast::Receiver<Event>,
) {
    let bus = EventBus::default();
    let events = bus.subscribe();
    let connections = Connections::new(bus);
    let fake = Arc::new(Fake::default());
    connections.register(&device(), fake.clone()).await;
    (connections, fake, events)
}
async fn receive(events: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(4), events.recv())
        .await
        .expect("event deadline")
        .expect("event")
}
async fn packet(fake: &Fake, bytes: &[u8]) {
    let sender = fake
        .sender
        .lock()
        .expect("mutex")
        .as_ref()
        .expect("session")
        .clone();
    sender.send(bytes.to_vec()).await.expect("packet");
    tokio::task::yield_now().await;
}

#[tokio::test(start_paused = true)]
async fn connection_is_idempotent_and_split_records_do_not_survive_reconnect() {
    let (connections, fake, mut events) = fixture().await;
    let (first, second) = tokio::join!(
        connections.set_connection("ble-fixture", true),
        connections.set_connection("ble-fixture", true)
    );
    assert_eq!(first.expect("first"), second.expect("second"));
    assert_eq!(fake.opens.load(SeqCst), 1);
    assert!(
        matches!(receive(&mut events).await,Event::DeviceConnected {data} if data.capabilities.contains(&DeviceCapability::Power))
    );
    let mut updated = device();
    updated.name = "Renamed".into();
    updated.signal_strength = Some(-70);
    connections.register(&updated, fake.clone()).await;
    connections.overlay(&mut updated).await;
    assert!(updated.connected);
    assert!(updated.capabilities.contains(&DeviceCapability::Cadence));
    packet(&fake, &[0x41, 0, 250, 0]).await;
    packet(&fake, &[4, 0, 0xb8, 0xb, 181, 0]).await;
    assert!(
        matches!(receive(&mut events).await,Event::Telemetry {data,..} if data.power_watts == Some(250) && data.cadence_rpm == Some(90.5) && data.speed_kph == Some(30.0))
    );
    packet(&fake, &[0x41, 0, 99, 0]).await;
    connections
        .set_connection("ble-fixture", false)
        .await
        .expect("disconnect");
    assert!(matches!(
        receive(&mut events).await,
        Event::DeviceDisconnected { .. }
    ));
    connections
        .set_connection("ble-fixture", false)
        .await
        .expect("idempotent disconnect");
    assert_eq!(fake.closes.load(SeqCst), 1);
    connections
        .set_connection("ble-fixture", true)
        .await
        .expect("reconnect");
    assert!(matches!(
        receive(&mut events).await,
        Event::DeviceConnected { .. }
    ));
    packet(&fake, &[0, 0, 0, 0]).await;
    assert!(
        matches!(receive(&mut events).await,Event::Telemetry {data,..} if data.power_watts.is_none())
    );
    connections.shutdown().await;
    assert!(!fake.connected.load(SeqCst));
    assert_eq!(fake.closes.load(SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn setup_errors_and_timeout_cleanup_partial_connections_and_allow_retry() {
    for (mode, code) in [
        (1, ErrorCode::UnsupportedOperation),
        (2, ErrorCode::Timeout),
        (3, ErrorCode::InvalidDeviceData),
    ] {
        let (connections, fake, mut events) = fixture().await;
        fake.open_mode.store(mode, SeqCst);
        assert_eq!(
            connections
                .set_connection("ble-fixture", true)
                .await
                .expect_err("failed open")
                .code,
            code
        );
        assert!(!fake.connected.load(SeqCst));
        assert_eq!(fake.closes.load(SeqCst), 1);
        assert!(matches!(receive(&mut events).await,Event::Error{data} if data.code == code));
        assert!(events.try_recv().is_err());
        fake.open_mode.store(0, SeqCst);
        connections
            .set_connection("ble-fixture", true)
            .await
            .expect("retry");
        connections.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn stream_end_and_link_loss_disconnect_without_auto_reconnecting() {
    for stream_end in [false, true] {
        let (connections, fake, mut events) = fixture().await;
        connections
            .set_connection("ble-fixture", true)
            .await
            .expect("connect");
        receive(&mut events).await;
        if stream_end {
            fake.sender.lock().expect("mutex").take();
        } else {
            fake.connected.store(false, SeqCst);
        }
        assert!(
            matches!(receive(&mut events).await,Event::Error{data} if data.code == ErrorCode::DeviceDisconnected)
        );
        assert!(matches!(
            receive(&mut events).await,
            Event::DeviceDisconnected { .. }
        ));
        assert_eq!(fake.opens.load(SeqCst), 1);
        let mut info = device();
        connections.overlay(&mut info).await;
        assert!(!info.connected);
        connections.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_and_request_cancellation_clean_up_an_inflight_open() {
    for shutdown in [true, false] {
        let (connections, fake, _) = fixture().await;
        fake.open_mode.store(2, SeqCst);
        let cloned = connections.clone();
        let request = tokio::spawn(async move { cloned.set_connection("ble-fixture", true).await });
        while fake.opens.load(SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        if shutdown {
            connections.shutdown().await;
            assert!(request.await.expect("join").is_err());
        } else {
            request.abort();
            let _ = request.await;
            while fake.closes.load(SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            connections.shutdown().await;
        }
        assert!(!fake.connected.load(SeqCst));
        assert_eq!(fake.closes.load(SeqCst), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn failed_teardown_is_visible_and_retried_before_open() {
    let (connections, fake, mut events) = fixture().await;
    connections
        .set_connection("ble-fixture", true)
        .await
        .expect("connect");
    receive(&mut events).await;
    fake.fail_close.store(true, SeqCst);
    assert!(
        connections
            .set_connection("ble-fixture", false)
            .await
            .is_err()
    );
    assert!(matches!(
        receive(&mut events).await,
        Event::DeviceDisconnected { .. }
    ));
    assert!(matches!(receive(&mut events).await, Event::Error { .. }));
    assert!(
        connections
            .set_connection("ble-fixture", true)
            .await
            .is_err()
    );
    assert_eq!(fake.opens.load(SeqCst), 1);
    fake.fail_close.store(false, SeqCst);
    connections
        .set_connection("ble-fixture", true)
        .await
        .expect("retry after recovery");
    assert_eq!(fake.opens.load(SeqCst), 2);
    connections.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cycling_power_verifies_role_streams_telemetry_and_resets_on_reconnect() {
    let (connections, fake, mut events) = fixture().await;
    fake.cycling_power.store(true, SeqCst);
    let info = connections
        .set_connection("ble-fixture", true)
        .await
        .expect("connect CPS");
    assert_eq!(info.kind, DeviceKind::PowerMeter);
    assert_eq!(
        info.capabilities,
        vec![DeviceCapability::Power, DeviceCapability::Cadence]
    );
    assert!(matches!(
        receive(&mut events).await,
        Event::DeviceConnected { .. }
    ));
    let mut stale = device();
    stale.kind = DeviceKind::Unknown;
    connections.register(&stale, fake.clone()).await;
    connections.overlay(&mut stale).await;
    assert_eq!(stale.kind, DeviceKind::PowerMeter);
    packet(&fake, &[32, 0, 250, 0, 1, 0, 0, 4]).await;
    assert!(
        matches!(receive(&mut events).await, Event::Telemetry {data, ..} if data.power_watts == Some(250) && data.cadence_rpm.is_none())
    );
    packet(&fake, &[32, 0, 251, 0, 2, 0, 0, 8]).await;
    assert!(
        matches!(receive(&mut events).await, Event::Telemetry {data, ..} if data.power_watts == Some(251) && data.cadence_rpm == Some(60.0) && data.speed_kph.is_none())
    );
    let error = connections
        .execute(1, "ble-fixture", TrainerCommand::SetTargetPower(250))
        .await
        .expect_err("CPS cannot control load");
    assert_eq!(error.code, ErrorCode::UnsupportedOperation);
    assert!(fake.writes.lock().expect("mutex").is_empty());
    assert!(matches!(receive(&mut events).await, Event::Error { .. }));
    connections
        .set_connection("ble-fixture", false)
        .await
        .expect("disconnect");
    assert!(matches!(
        receive(&mut events).await,
        Event::DeviceDisconnected { .. }
    ));
    connections
        .set_connection("ble-fixture", true)
        .await
        .expect("reconnect");
    assert!(matches!(
        receive(&mut events).await,
        Event::DeviceConnected { .. }
    ));
    packet(&fake, &[32, 0, 252, 0, 3, 0, 0, 12]).await;
    assert!(
        matches!(receive(&mut events).await, Event::Telemetry {data, ..} if data.cadence_rpm.is_none())
    );
    connections.shutdown().await;
}
