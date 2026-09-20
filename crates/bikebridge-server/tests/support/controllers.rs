use super::*;
use bikebridge_core::{DeviceCapability, Event, InputState};
use bikebridge_openbikecontrol::{ControllerTransport, SERVICE};
use futures_util::{
    future::BoxFuture,
    stream::{self, BoxStream},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering::SeqCst},
};

#[derive(Default)]
struct ControllerFixture {
    connected: AtomicBool,
    sender: Mutex<Option<tokio::sync::mpsc::Sender<Vec<u8>>>>,
}
impl ControllerTransport for ControllerFixture {
    fn open(&self) -> BoxFuture<'_, bikebridge_core::Result<BoxStream<'static, Vec<u8>>>> {
        Box::pin(async {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            *self.sender.lock().expect("mutex") = Some(tx);
            self.connected.store(true, SeqCst);
            Ok(
                stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|p| (p, rx)) })
                    .boxed(),
            )
        })
    }
    fn is_connected(&self) -> BoxFuture<'_, bikebridge_core::Result<bool>> {
        Box::pin(async { Ok(self.connected.load(SeqCst)) })
    }
    fn disconnect(&self) -> BoxFuture<'_, bikebridge_core::Result<()>> {
        Box::pin(async {
            self.connected.store(false, SeqCst);
            self.sender.lock().expect("mutex").take();
            Ok(())
        })
    }
}
struct ControllerDiscovery {
    fixture: Arc<ControllerFixture>,
    base: DiscoveryFixture,
}
impl bikebridge_ble::DiscoveryBackend for ControllerDiscovery {
    fn controller(&self, _: &str, _: &str) -> Option<Arc<dyn ControllerTransport>> {
        Some(self.fixture.clone())
    }
    async fn adapters(&mut self) -> bikebridge_core::Result<Vec<bikebridge_ble::BackendAdapter>> {
        self.base.adapters().await
    }
    async fn start_scan(&mut self, id: &str) -> bikebridge_core::Result<()> {
        self.base.start_scan(id).await
    }
    async fn stop_scan(&mut self, id: &str) -> bikebridge_core::Result<()> {
        self.base.stop_scan(id).await
    }
    async fn advertisements(
        &mut self,
        _: &str,
    ) -> bikebridge_core::Result<Vec<bikebridge_ble::Advertisement>> {
        Ok(vec![bikebridge_ble::Advertisement {
            key: "private-controller".into(),
            name: Some("BikeControl".into()),
            rssi: Some(-50),
            services: vec![SERVICE],
        }])
    }
}

#[tokio::test]
async fn controller_bytes_to_websocket_recording_and_replay_with_disconnect_release() {
    let state = AppState::new(false, SafetyLimits::default()).expect("state");
    let path = std::env::temp_dir().join(format!(
        "bikebridge-controller-{}.biketrace",
        std::process::id()
    ));
    let recorder =
        bikebridge_trace::Recorder::start(&path, state.events.clone(), vec![]).expect("record");
    let fixture = Arc::new(ControllerFixture::default());
    let scanner = bikebridge_ble::Scanner::spawn(
        ControllerDiscovery {
            fixture: fixture.clone(),
            base: DiscoveryFixture::default(),
        },
        state.events.clone(),
        None,
    )
    .await;
    scanner.start().await.expect("scan");
    let id = timeout(Duration::from_secs(3), async {
        loop {
            if let Some(device) = scanner.snapshot().await.devices.first() {
                break device.id.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("discovery");
    let daemon = Daemon::with_state(state.with_scanner(scanner)).await;
    let host = daemon.address.to_string();
    let mut viewer = daemon.client().await;
    send(
        &mut viewer,
        json!({"type":"subscribe","requestId":"sub","events":["input","device"],"deviceIds":[id]}),
    )
    .await;
    assert_eq!(response(&mut viewer, "sub").await["success"], true);
    let (code, info) = daemon
        .request("POST", &format!("/api/devices/{id}/connect"), &host, "")
        .await;
    assert_eq!(code, 200);
    assert_eq!(info["kind"], "bike_controller");
    assert_eq!(
        info["capabilities"],
        json!([DeviceCapability::ControllerInput])
    );
    assert_eq!(
        event(&mut viewer, "device.connected").await["data"]["id"],
        id
    );
    let sender = fixture
        .sender
        .lock()
        .expect("mutex")
        .as_ref()
        .expect("open")
        .clone();
    sender
        .send(vec![1, 1, 1, 0x18, 1, 0x14, 1, 0x1a, 101])
        .await
        .expect("buttons");
    let mut received = Vec::new();
    for expected in ["shift_up", "steering_left", "confirm", "brake"] {
        let input = event(&mut viewer, "input").await;
        assert_eq!(input["data"]["input"], expected);
        received.push(input);
    }
    assert_eq!(received[3]["data"]["value"], 1.0);
    sender
        .send(vec![1, 1, 0, 1, 1, 1, 0])
        .await
        .expect("rapid shifting");
    for state in ["released", "pressed", "released"] {
        let input = event(&mut viewer, "input").await;
        assert_eq!(input["data"]["state"], state);
        received.push(input);
    }
    // It is a process-wide input session, with no trainer-control lease.
    let observer = daemon.client().await;
    drop(observer);
    assert!(fixture.connected.load(SeqCst));
    send(&mut viewer,json!({"type":"trainer.setTargetPower","requestId":"reject","deviceId":id,"data":{"watts":100}})).await;
    assert_eq!(
        response(&mut viewer, "reject").await["error"]["code"],
        "unsupported_operation"
    );
    assert_eq!(
        daemon
            .request("POST", &format!("/api/devices/{id}/disconnect"), &host, "")
            .await
            .0,
        200
    );
    for _ in 0..3 {
        received.push(event(&mut viewer, "input").await);
    }
    assert_eq!(
        event(&mut viewer, "device.disconnected").await["data"]["connected"],
        false
    );
    assert!(!fixture.connected.load(SeqCst));
    drop(sender);
    daemon.stop().await;
    tokio::task::spawn_blocking(move || recorder.finish())
        .await
        .expect("join")
        .expect("finalize");
    let trace = bikebridge_trace::Trace::load(&path).expect("trace");
    std::fs::remove_file(path).expect("cleanup");
    let mut playback = bikebridge_trace::Playback::new(trace, 1.0).expect("playback");
    playback.start();
    let events = playback.advance(Duration::from_secs(60));
    let replayed: Vec<Value> = events
        .iter()
        .filter(|e| matches!(e, Event::Input { .. }))
        .map(|e| serde_json::from_str(&serde_json::to_string(e).expect("encode")).expect("JSON"))
        .collect();
    assert_eq!(replayed, received);
    assert!(
        events
            .iter()
            .any(|e| matches!(e,Event::Input {data,..} if data.state==InputState::Released))
    );
}
