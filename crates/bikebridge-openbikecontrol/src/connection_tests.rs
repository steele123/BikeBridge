use super::*;
use bikebridge_core::{BikeInput, DeviceKind, InputState};
use futures_util::{future::BoxFuture, stream};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
};

#[derive(Default)]
struct Fake {
    opens: AtomicUsize,
    closes: AtomicUsize,
    connected: AtomicBool,
    fail: AtomicBool,
    hang: AtomicBool,
    sender: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
}
impl ControllerTransport for Fake {
    fn open(&self) -> BoxFuture<'_, Result<BoxStream<'static, Vec<u8>>>> {
        Box::pin(async {
            self.opens.fetch_add(1, SeqCst);
            if self.hang.load(SeqCst) {
                return std::future::pending().await;
            }
            if self.fail.load(SeqCst) {
                return Err(BridgeError::new(
                    ErrorCode::ConnectionFailed,
                    "fixture failed",
                ));
            }
            let (tx, rx) = mpsc::channel(16);
            *self.sender.lock().expect("mutex") = Some(tx);
            self.connected.store(true, SeqCst);
            Ok(
                stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|p| (p, rx)) })
                    .boxed(),
            )
        })
    }
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(self.connected.load(SeqCst)) })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async {
            self.closes.fetch_add(1, SeqCst);
            self.connected.store(false, SeqCst);
            self.sender.lock().expect("mutex").take();
            Ok(())
        })
    }
}
impl Fake {
    async fn packet(&self, bytes: &[u8]) {
        let sender = self
            .sender
            .lock()
            .expect("mutex")
            .as_ref()
            .expect("open")
            .clone();
        sender.send(bytes.to_vec()).await.expect("packet");
    }
}
fn info() -> DeviceInfo {
    DeviceInfo {
        id: "ble-controller".into(),
        name: "BikeControl".into(),
        kind: DeviceKind::BikeController,
        transport: "bluetooth".into(),
        connected: false,
        signal_strength: None,
        capabilities: vec![],
    }
}
async fn setup() -> (
    Controllers,
    Arc<Fake>,
    tokio::sync::broadcast::Receiver<Event>,
) {
    let bus = EventBus::default();
    let events = bus.subscribe();
    let controllers = Controllers::new(bus);
    let fake = Arc::new(Fake::default());
    controllers.register(&info(), fake.clone()).await;
    (controllers, fake, events)
}
async fn next(events: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(45), events.recv())
        .await
        .expect("timeout")
        .expect("event")
}
async fn kind(events: &mut tokio::sync::broadcast::Receiver<Event>, category: &str) -> Event {
    loop {
        let event = next(events).await;
        if event.category() == category {
            return event;
        }
    }
}
#[tokio::test(start_paused = true)]
async fn verified_connection_edges_and_shutdown_release_held_buttons() {
    let (controllers, fake, mut events) = setup().await;
    let connected = controllers
        .set_connection("ble-controller", true)
        .await
        .expect("connect");
    assert_eq!(connected.capabilities, [DeviceCapability::ControllerInput]);
    assert!(matches!(
        next(&mut events).await,
        Event::DeviceConnected { .. }
    ));
    controllers
        .set_connection("ble-controller", true)
        .await
        .expect("idempotent");
    assert_eq!(fake.opens.load(SeqCst), 1);
    fake.packet(&[1, 1, 1]).await;
    assert!(
        matches!(kind(&mut events,"input").await,Event::Input {data,..} if data.input==BikeInput::ShiftUp && data.state==InputState::Pressed)
    );
    controllers.shutdown().await;
    assert!(
        matches!(next(&mut events).await,Event::Input {data,..} if data.state==InputState::Released)
    );
    assert!(matches!(
        next(&mut events).await,
        Event::DeviceDisconnected { .. }
    ));
    assert_eq!(fake.closes.load(SeqCst), 1);
}
#[tokio::test(start_paused = true)]
async fn loss_releases_before_reconnect_and_manual_disconnect_cancels_retries() {
    let (controllers, fake, mut events) = setup().await;
    controllers
        .set_connection("ble-controller", true)
        .await
        .expect("connect");
    next(&mut events).await;
    fake.packet(&[1, 0x18, 1]).await;
    kind(&mut events, "input").await;
    fake.sender.lock().expect("mutex").take();
    assert!(
        matches!(kind(&mut events,"input").await,Event::Input {data,..} if data.state==InputState::Released)
    );
    assert!(matches!(
        next(&mut events).await,
        Event::DeviceDisconnected { .. }
    ));
    assert!(matches!(
        next(&mut events).await,
        Event::DeviceReconnecting { attempt: 1, .. }
    ));
    assert!(matches!(
        next(&mut events).await,
        Event::DeviceConnected { .. }
    ));
    assert_eq!(fake.opens.load(SeqCst), 2);
    assert!(events.try_recv().is_err());
    fake.packet(&[1, 0x18, 1]).await;
    assert!(
        matches!(kind(&mut events,"input").await,Event::Input {data,..} if data.state==InputState::Pressed)
    );
    fake.sender.lock().expect("mutex").take();
    loop {
        if matches!(next(&mut events).await, Event::DeviceReconnecting { .. }) {
            break;
        }
    }
    controllers
        .set_connection("ble-controller", false)
        .await
        .expect("disconnect");
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert_eq!(fake.opens.load(SeqCst), 2);
    controllers.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn malformed_packet_releases_and_stops_without_retrying() {
    let (controllers, fake, mut events) = setup().await;
    controllers
        .set_connection("ble-controller", true)
        .await
        .expect("connect");
    next(&mut events).await;
    fake.packet(&[1, 1, 1]).await;
    kind(&mut events, "input").await;
    fake.packet(&[1, 1]).await;
    assert!(
        matches!(next(&mut events).await,Event::Error {data} if data.code==ErrorCode::InvalidDeviceData)
    );
    assert!(
        matches!(next(&mut events).await,Event::Input {data,..} if data.state==InputState::Released)
    );
    assert!(matches!(
        next(&mut events).await,
        Event::DeviceDisconnected { .. }
    ));
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert_eq!(fake.opens.load(SeqCst), 1);
    controllers.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn timeout_and_cancelled_setup_always_disconnect() {
    let (controllers, fake, _) = setup().await;
    fake.hang.store(true, SeqCst);
    assert_eq!(
        controllers
            .set_connection("ble-controller", true)
            .await
            .expect_err("timeout")
            .code,
        ErrorCode::Timeout
    );
    tokio::task::yield_now().await;
    assert_eq!(fake.closes.load(SeqCst), 1);
    let task = tokio::spawn({
        let c = controllers.clone();
        async move { c.set_connection("ble-controller", true).await }
    });
    while fake.opens.load(SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    task.abort();
    let _ = task.await;
    while fake.closes.load(SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    controllers.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn retry_budget_is_five_and_never_replays_held_input() {
    let (controllers, fake, mut events) = setup().await;
    controllers
        .set_connection("ble-controller", true)
        .await
        .expect("connect");
    next(&mut events).await;
    fake.fail.store(true, SeqCst);
    fake.sender.lock().expect("mutex").take();
    for (index, delay) in [1, 2, 5, 10, 30].into_iter().enumerate() {
        loop {
            if let Event::DeviceReconnecting {
                attempt,
                delay_seconds,
                ..
            } = next(&mut events).await
            {
                assert_eq!(attempt, index as u8 + 1);
                assert_eq!(delay_seconds, delay);
                break;
            }
        }
    }
    loop {
        if fake.opens.load(SeqCst) == 6 {
            break;
        }
        next(&mut events).await;
    }
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert_eq!(fake.opens.load(SeqCst), 6);
    controllers.shutdown().await;
}
