//! Real loopback HTTP/WebSocket integration tests; no Bluetooth hardware required.
#[path = "support/controllers.rs"]
mod controllers;
#[path = "support/ftms_control.rs"]
mod ftms_control;
#[path = "support/trace.rs"]
mod trace;
use bikebridge_core::SafetyLimits;
use bikebridge_server::AppState;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Daemon {
    address: SocketAddr,
    state: AppState,
    shutdown: CancellationToken,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl Daemon {
    async fn start(mock: bool) -> Self {
        Self::with_state(AppState::new(mock, SafetyLimits::default()).expect("state")).await
    }
    async fn with_state(state: AppState) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(bikebridge_server::serve(
            listener,
            state.clone(),
            shutdown.clone(),
        ));
        Self {
            address,
            state,
            shutdown,
            task: Some(task),
        }
    }
    async fn client(&self) -> Client {
        let (mut client, _) = connect_async(format!("ws://{}/ws", self.address))
            .await
            .expect("connect");
        let hello = next_json(&mut client).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["protocolVersion"], 1);
        client
    }
    async fn http(&self, path: &str, host: &str, extra: &str) -> (u16, Value) {
        self.request("GET", path, host, extra).await
    }
    async fn request(&self, method: &str, path: &str, host: &str, extra: &str) -> (u16, Value) {
        timeout(Duration::from_secs(3), async {
            let mut stream = TcpStream::connect(self.address).await.expect("connect");
            stream
                .write_all(format!("{method} {path} HTTP/1.0\r\nHost: {host}\r\nContent-Length: 0\r\n{extra}\r\n").as_bytes())
                .await
                .expect("write");
            let mut response = String::new();
            stream.read_to_string(&mut response).await.expect("read");
            let (head, body) = response.split_once("\r\n\r\n").expect("HTTP");
            let status = head
                .split_whitespace()
                .nth(1)
                .expect("status")
                .parse()
                .expect("code");
            (status, serde_json::from_str(body).expect("JSON"))
        })
        .await
        .expect("HTTP timeout")
    }
    async fn stop(mut self) {
        self.shutdown.cancel();
        timeout(Duration::from_secs(5), self.task.take().expect("task"))
            .await
            .expect("shutdown timeout")
            .expect("join")
            .expect("serve");
        assert_eq!(self.state.status().await.websocket_clients, 0);
    }
}

#[derive(Clone, Default)]
struct DiscoveryFixture {
    transport: Option<std::sync::Arc<TelemetryFixture>>,
    counts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    rssi: std::sync::Arc<std::sync::atomic::AtomicI16>,
    absent: bool,
}

impl bikebridge_ble::DiscoveryBackend for DiscoveryFixture {
    fn peripheral(
        &self,
        _: &str,
        _: &str,
    ) -> Option<std::sync::Arc<dyn bikebridge_ble::transport::FtmsTransport>> {
        self.transport.clone().map(|transport| {
            transport as std::sync::Arc<dyn bikebridge_ble::transport::FtmsTransport>
        })
    }
    async fn adapters(&mut self) -> bikebridge_core::Result<Vec<bikebridge_ble::BackendAdapter>> {
        Ok(if self.absent {
            Vec::new()
        } else {
            vec![bikebridge_ble::BackendAdapter {
                key: "platform-private-adapter".into(),
                state: bikebridge_core::AdapterState::PoweredOn,
            }]
        })
    }
    async fn start_scan(&mut self, _: &str) -> bikebridge_core::Result<()> {
        self.counts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn stop_scan(&mut self, _: &str) -> bikebridge_core::Result<()> {
        self.counts
            .fetch_add(10, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn advertisements(
        &mut self,
        _: &str,
    ) -> bikebridge_core::Result<Vec<bikebridge_ble::Advertisement>> {
        Ok(vec![bikebridge_ble::Advertisement {
            key: "platform-private-device".into(),
            name: Some("Discovery Trainer".into()),
            rssi: Some(self.rssi.load(std::sync::atomic::Ordering::SeqCst)),
            services: vec![bikebridge_ble::classification::FITNESS_MACHINE],
        }])
    }
}

#[derive(Default)]
struct TelemetryFixture {
    control: bool,
    hold_response: std::sync::atomic::AtomicBool,
    control_sender: std::sync::Mutex<
        Option<tokio::sync::mpsc::Sender<bikebridge_ble::transport::ControlEvent>>,
    >,
    writes: std::sync::Mutex<Vec<Vec<u8>>>,
    sender: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<Vec<u8>>>>,
    opened: std::sync::atomic::AtomicUsize,
    closed: std::sync::atomic::AtomicUsize,
    gate: Option<std::sync::Arc<tokio::sync::Semaphore>>,
}
impl bikebridge_ble::transport::FtmsTransport for TelemetryFixture {
    fn write_control<'a>(
        &'a self,
        bytes: &'a [u8],
    ) -> futures_util::future::BoxFuture<'a, bikebridge_core::Result<()>> {
        Box::pin(async move {
            self.writes.lock().expect("mutex").push(bytes.to_vec());
            if !self.hold_response.load(std::sync::atomic::Ordering::SeqCst) {
                self.acknowledge(bytes[0]).await;
            }
            Ok(())
        })
    }
    fn open(
        &self,
    ) -> futures_util::future::BoxFuture<
        '_,
        bikebridge_core::Result<bikebridge_ble::transport::FtmsSession>,
    > {
        Box::pin(async {
            self.opened
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.acquire().await.expect("permit").forget();
            }
            let (sender, receiver) = tokio::sync::mpsc::channel(8);
            *self.sender.lock().expect("mutex") = Some(sender);
            let control = if self.control {
                let (sender, receiver) = tokio::sync::mpsc::channel(32);
                *self.control_sender.lock().expect("mutex") = Some(sender);
                Some(bikebridge_ble::transport::ControlSession {
                    profile: bikebridge_ble::control::ControlProfile::discover(
                        &[2, 64, 0, 0, 12, 32, 0, 0],
                        Some(&[0, 0, 232, 3, 10, 0]),
                        Some(&[0, 0, 220, 5, 5, 0]),
                    )
                    .expect("profile"),
                    events: Box::pin(futures_util::stream::unfold(
                        receiver,
                        |mut receiver| async {
                            receiver.recv().await.map(|event| (event, receiver))
                        },
                    )),
                })
            } else {
                None
            };
            Ok(bikebridge_ble::transport::FtmsSession {
                control,
                features: vec![2, 0x40, 0, 0, 255, 255, 255, 255],
                notifications: Box::pin(futures_util::stream::unfold(
                    receiver,
                    |mut receiver| async { receiver.recv().await.map(|packet| (packet, receiver)) },
                )),
            })
        })
    }
    fn is_connected(&self) -> futures_util::future::BoxFuture<'_, bikebridge_core::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn disconnect(&self) -> futures_util::future::BoxFuture<'_, bikebridge_core::Result<()>> {
        Box::pin(async {
            self.closed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.sender.lock().expect("mutex").take();
            self.control_sender.lock().expect("mutex").take();
            Ok(())
        })
    }
}
impl TelemetryFixture {
    async fn acknowledge(&self, opcode: u8) {
        let sender = self
            .control_sender
            .lock()
            .expect("mutex")
            .as_ref()
            .expect("control stream")
            .clone();
        let _ = sender
            .send(bikebridge_ble::transport::ControlEvent::Indication(vec![
                128, opcode, 1,
            ]))
            .await;
    }
    async fn packet(&self, bytes: &[u8]) {
        let sender = self
            .sender
            .lock()
            .expect("mutex")
            .as_ref()
            .expect("session")
            .clone();
        sender.send(bytes.to_vec()).await.expect("packet");
    }
}
async fn telemetry_daemon(
    transport: std::sync::Arc<TelemetryFixture>,
) -> (Daemon, DiscoveryFixture, String) {
    let fixture = DiscoveryFixture {
        transport: Some(transport),
        ..Default::default()
    };
    let state = AppState::new(false, SafetyLimits::default()).expect("state");
    let scanner = bikebridge_ble::Scanner::spawn(fixture.clone(), state.events.clone(), None).await;
    scanner.start().await.expect("scan");
    let id = timeout(Duration::from_secs(3), async {
        loop {
            if let Some(device) = scanner.snapshot().await.devices.first() {
                break device.id.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("discovery");
    (
        Daemon::with_state(state.with_scanner(scanner)).await,
        fixture,
        id,
    )
}

#[tokio::test]
async fn ftms_bytes_to_http_connection_and_websocket_telemetry() {
    use std::sync::{Arc, atomic::Ordering::SeqCst};
    let transport = Arc::new(TelemetryFixture::default());
    let (daemon, fixture, id) = telemetry_daemon(transport.clone()).await;
    let host = daemon.address.to_string();
    let mut client = daemon.client().await;
    send(&mut client,json!({"type":"subscribe","requestId":"sub","events":["device","telemetry","error"],"deviceIds":[id]})).await;
    response(&mut client, "sub").await;
    assert_eq!(
        daemon
            .request(
                "POST",
                &format!("/api/devices/{id}/connect"),
                &host,
                "Origin: https://example.com\r\n"
            )
            .await
            .0,
        403
    );
    assert_eq!(transport.opened.load(SeqCst), 0);
    assert_eq!(
        daemon
            .request("POST", "/api/devices/missing/connect", &host, "")
            .await
            .0,
        404
    );
    let (status, connected) = daemon
        .request("POST", &format!("/api/devices/{id}/connect"), &host, "")
        .await;
    assert_eq!(status, 200);
    assert_eq!(connected["connected"], true);
    assert_eq!(
        connected["capabilities"],
        json!(["speed", "cadence", "power"])
    );
    assert_eq!(
        event(&mut client, "device.connected").await["data"],
        connected
    );
    fixture.rssi.store(-65, SeqCst);
    let updated = event(&mut client, "device.updated").await;
    assert_eq!(updated["data"]["connected"], true);
    assert_eq!(updated["data"]["capabilities"], connected["capabilities"]);
    assert_eq!(daemon.state.status().await.connected_devices, 1);
    // One measurement fragmented across notifications, using actual encoded fields.
    transport.packet(&[0x41, 0, 250, 0]).await;
    transport.packet(&[4, 0, 0xb8, 0xb, 181, 0]).await;
    let telemetry = event(&mut client, "telemetry").await;
    assert_eq!(telemetry["deviceId"], id);
    assert_eq!(telemetry["data"]["powerWatts"], 250);
    assert_eq!(telemetry["data"]["cadenceRpm"], 90.5);
    assert_eq!(telemetry["data"]["speedKph"], 30.0);
    assert!(telemetry["data"]["timestampMs"].as_u64().expect("time") > 0);
    assert!(telemetry["data"].get("resistanceLevel").is_none());
    send(&mut client,json!({"type":"trainer.setTargetPower","requestId":"control","deviceId":id,"data":{"watts":250}})).await;
    assert_eq!(
        response(&mut client, "control").await["error"]["code"],
        "unsupported_operation"
    );
    // A second observer can leave without disconnecting the process-wide BLE session.
    let observer = daemon.client().await;
    drop(observer);
    assert_eq!(transport.closed.load(SeqCst), 0);
    assert_eq!(
        daemon
            .request("POST", &format!("/api/devices/{id}/disconnect"), &host, "")
            .await
            .0,
        200
    );
    assert_eq!(
        event(&mut client, "device.disconnected").await["data"]["connected"],
        false
    );
    assert_eq!(daemon.state.status().await.connected_devices, 0);
    send(
        &mut client,
        json!({"type":"device.connect","deviceId":id,"requestId":"again"}),
    )
    .await;
    assert_eq!(response(&mut client, "again").await["success"], true);
    transport.packet(&[0, 0, 0, 0]).await;
    assert!(
        event(&mut client, "telemetry").await["data"]
            .get("powerWatts")
            .is_none()
    );
    daemon.stop().await;
    assert_eq!(transport.opened.load(SeqCst), 2);
    assert_eq!(transport.closed.load(SeqCst), 2);
}

#[tokio::test]
async fn pending_ble_connection_does_not_block_websocket_or_http() {
    use std::sync::{Arc, atomic::Ordering::SeqCst};
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let transport = Arc::new(TelemetryFixture {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let (daemon, _, id) = telemetry_daemon(transport.clone()).await;
    let mut client = daemon.client().await;
    send(
        &mut client,
        json!({"type":"device.connect","deviceId":id,"requestId":"connect"}),
    )
    .await;
    send(
        &mut client,
        json!({"type":"subscribe","events":["telemetry"],"requestId":"sub"}),
    )
    .await;
    assert_eq!(response(&mut client, "sub").await["success"], true);
    send(
        &mut client,
        json!({"type":"device.connect","deviceId":id,"requestId":"busy"}),
    )
    .await;
    assert_eq!(response(&mut client, "busy").await["error"]["code"], "busy");
    assert_eq!(
        daemon
            .http("/api/status", &daemon.address.to_string(), "")
            .await
            .0,
        200
    );
    assert_eq!(
        daemon
            .request("POST", "/api/scan/stop", &daemon.address.to_string(), "")
            .await
            .0,
        200
    );
    gate.add_permits(1);
    assert_eq!(response(&mut client, "connect").await["success"], true);
    assert_eq!(transport.opened.load(SeqCst), 1);
    transport
        .packet(&[0x44, 0, 0xb8, 0xb, 180, 0, 250, 0])
        .await;
    assert_eq!(
        event(&mut client, "telemetry").await["data"]["powerWatts"],
        250
    );
    daemon.stop().await;
}

#[tokio::test]
async fn http_connection_commands_cannot_bypass_mock_control_ownership() {
    let daemon = Daemon::start(true).await;
    let mut owner = daemon.client().await;
    send(
        &mut owner,
        json!({"type":"trainer.requestControl","deviceId":"mock-trainer","requestId":"owner"}),
    )
    .await;
    assert_eq!(response(&mut owner, "owner").await["success"], true);
    let (status, error) = daemon
        .request(
            "POST",
            "/api/devices/mock-trainer/disconnect",
            &daemon.address.to_string(),
            "",
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(error["data"]["code"], "trainer_control_denied");
    daemon.stop().await;
}

#[tokio::test]
async fn scan_http_to_discovery_websocket_and_device_snapshot() {
    let fixture = DiscoveryFixture::default();
    fixture.rssi.store(-52, std::sync::atomic::Ordering::SeqCst);
    let state = AppState::new(false, SafetyLimits::default()).expect("state");
    let scanner = bikebridge_ble::Scanner::spawn(fixture.clone(), state.events.clone(), None).await;
    let daemon = Daemon::with_state(state.with_scanner(scanner)).await;
    let host = daemon.address.to_string();
    let mut client = daemon.client().await;
    send(
        &mut client,
        json!({"type":"subscribe","requestId":"sub","events":["device","scan","error"]}),
    )
    .await;
    assert_eq!(response(&mut client, "sub").await["success"], true);
    let (_, adapters) = daemon.http("/api/adapters", &host, "").await;
    assert_eq!(adapters[0]["state"], "powered_on");
    assert_eq!(adapters[0]["isDefault"], true);
    assert!(!adapters.to_string().contains("platform-private"));
    assert_eq!(
        daemon
            .request(
                "POST",
                "/api/scan/start",
                &host,
                "Origin: https://example.com\r\n"
            )
            .await
            .0,
        403
    );
    let (status, scan) = daemon.request("POST", "/api/scan/start", &host, "").await;
    assert_eq!(status, 200);
    assert_eq!(scan["scanning"], true);
    assert_eq!(
        event(&mut client, "scan.status").await["data"]["scanning"],
        true
    );
    let discovery = event(&mut client, "device.discovered").await;
    let id = discovery["data"]["id"].as_str().expect("id");
    assert_eq!(discovery["data"]["kind"], "trainer");
    assert_eq!(discovery["data"]["connected"], false);
    assert_eq!(discovery["data"]["capabilities"], json!([]));
    assert!(!discovery.to_string().contains("platform-private"));
    let (_, snapshot) = daemon.http(&format!("/api/devices/{id}"), &host, "").await;
    assert_eq!(snapshot, discovery["data"]);
    let (_, status) = daemon.http("/api/status", &host, "").await;
    assert_eq!(status["bluetoothAvailable"], true);
    assert_eq!(status["bluetoothEnabled"], true);
    assert_eq!(status["connectedDevices"], 0);
    send(
        &mut client,
        json!({"type":"device.connect","requestId":"later","deviceId":id}),
    )
    .await;
    assert_eq!(
        response(&mut client, "later").await["error"]["code"],
        "unsupported_operation"
    );
    fixture.rssi.store(-60, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        event(&mut client, "device.updated").await["data"]["signalStrength"],
        -60
    );
    assert_eq!(
        daemon.request("POST", "/api/scan/stop", &host, "").await.0,
        200
    );
    assert_eq!(
        event(&mut client, "scan.status").await["data"]["scanning"],
        false
    );
    assert_eq!(
        daemon.request("POST", "/api/scan/start", &host, "").await.0,
        200
    );
    daemon.stop().await;
    assert_eq!(
        fixture.counts.load(std::sync::atomic::Ordering::SeqCst),
        22,
        "two starts and two stops, including shutdown"
    );
}

#[tokio::test]
async fn scan_reports_absent_adapter_and_mock_mode_separately() {
    let state = AppState::new(false, SafetyLimits::default()).expect("state");
    let scanner = bikebridge_ble::Scanner::spawn(
        DiscoveryFixture {
            absent: true,
            ..Default::default()
        },
        state.events.clone(),
        None,
    )
    .await;
    let daemon = Daemon::with_state(state.with_scanner(scanner)).await;
    let host = daemon.address.to_string();
    let (status, error) = daemon.request("POST", "/api/scan/start", &host, "").await;
    assert_eq!(status, 503);
    assert_eq!(error["data"]["code"], "adapter_not_found");
    let (_, status) = daemon.http("/api/status", &host, "").await;
    assert_eq!(status["bluetoothAvailable"], false);
    assert_eq!(status["scan"]["lastError"]["code"], "adapter_not_found");
    daemon.stop().await;
    let mock = Daemon::start(true).await;
    let (_, error) = mock
        .request("POST", "/api/scan/start", &mock.address.to_string(), "")
        .await;
    assert_eq!(error["data"]["code"], "bluetooth_unavailable");
    assert!(!mock.state.status().await.bluetooth_enabled);
    mock.stop().await;
}
impl Drop for Daemon {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn next_json(client: &mut Client) -> Value {
    timeout(Duration::from_secs(3), async {
        loop {
            match client.next().await.expect("socket open").expect("message") {
                Message::Text(text) => return serde_json::from_str(&text).expect("JSON"),
                Message::Ping(data) => client.send(Message::Pong(data)).await.expect("pong"),
                other => panic!("Unexpected message {other:?}"),
            }
        }
    })
    .await
    .expect("message timeout")
}
async fn send(client: &mut Client, value: Value) {
    client
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send");
}
async fn response(client: &mut Client, id: &str) -> Value {
    timeout(Duration::from_secs(3), async {
        loop {
            let value = next_json(client).await;
            if value["type"] == "response" && value["requestId"] == id {
                return value;
            }
        }
    })
    .await
    .expect("response timeout")
}
async fn event(client: &mut Client, kind: &str) -> Value {
    timeout(Duration::from_secs(3), async {
        loop {
            let value = next_json(client).await;
            if value["type"] == kind {
                return value;
            }
        }
    })
    .await
    .expect("event timeout")
}

// A telemetry sample may already be queued when a command is acknowledged.
// Wait for the new state rather than assuming the next sample is post-command.
async fn telemetry_matching(client: &mut Client, predicate: impl Fn(&Value) -> bool) -> Value {
    timeout(Duration::from_secs(3), async {
        loop {
            let sample = event(client, "telemetry").await;
            if predicate(&sample["data"]) {
                return sample;
            }
        }
    })
    .await
    .expect("updated telemetry timeout")
}

#[tokio::test]
async fn vertical_slice_telemetry_erg_and_controller_input() {
    let daemon = Daemon::start(true).await;
    let mut client = daemon.client().await;
    send(
        &mut client,
        json!({"type":"subscribe","requestId":"sub","events":["telemetry","input"]}),
    )
    .await;
    assert_eq!(response(&mut client, "sub").await["success"], true);
    let telemetry = event(&mut client, "telemetry").await;
    assert_eq!(telemetry["deviceId"], "mock-trainer");
    assert!(telemetry["data"]["powerWatts"].as_i64().expect("watts") > 0);
    send(&mut client, json!({"type":"trainer.setTargetPower","requestId":"erg","deviceId":"mock-trainer","data":{"watts":250}})).await;
    let reply = response(&mut client, "erg").await;
    assert_eq!(reply["success"], true);
    assert_eq!(reply["data"]["applied"]["value"], 250);
    telemetry_matching(&mut client, |data| {
        data["powerWatts"]
            .as_i64()
            .is_some_and(|watts| (247..=253).contains(&watts))
    })
    .await;
    send(&mut client, json!({"type":"mock.input","requestId":"shift","deviceId":"mock-controller","data":{"input":"shift_up","state":"pressed"}})).await;
    assert_eq!(response(&mut client, "shift").await["success"], true);
    let input = event(&mut client, "input").await;
    assert_eq!(input["deviceId"], "mock-controller");
    assert_eq!(input["data"]["input"], "shift_up");
    assert!(input["timestampMs"].as_u64().expect("timestamp") > 0);
    daemon.stop().await; // Active sockets must also shut down cleanly.
}

#[tokio::test]
async fn http_status_devices_and_loopback_security() {
    let daemon = Daemon::start(true).await;
    let host = daemon.address.to_string();
    let (status, body) = daemon.http("/api/status", &host, "").await;
    assert_eq!(status, 200);
    assert_eq!(body["bluetoothAvailable"], false);
    assert_eq!(body["connectedDevices"], 2);
    assert_eq!(body["mockMode"], true);
    let (_, devices) = daemon.http("/api/devices", &host, "").await;
    assert_eq!(devices.as_array().expect("devices").len(), 2);
    let (_, trainer) = daemon.http("/api/devices/mock-trainer", &host, "").await;
    assert_eq!(trainer["kind"], "trainer");
    assert_eq!(daemon.http("/api/devices/missing", &host, "").await.0, 404);
    assert_eq!(
        daemon
            .http("/api/status", &host, "Origin: https://example.com\r\n")
            .await
            .0,
        403
    );
    assert_eq!(
        daemon.http("/api/status", "attacker.example", "").await.0,
        403
    );
    let mut request = format!("ws://{}/ws", daemon.address)
        .into_client_request()
        .expect("request");
    request
        .headers_mut()
        .insert("Origin", "https://example.com".parse().expect("header"));
    assert!(connect_async(request).await.is_err());
    daemon.stop().await;
}

#[tokio::test]
async fn malformed_commands_do_not_change_trainer_and_limits_are_visible() {
    let daemon = Daemon::start(true).await;
    let mut client = daemon.client().await;
    client
        .send(Message::Text("{".into()))
        .await
        .expect("send invalid");
    assert_eq!(
        next_json(&mut client).await["error"]["code"],
        "invalid_command"
    );
    send(&mut client, json!({"type":"trainer.setTargetPower","requestId":"bad","deviceId":"mock-trainer","data":{"watts":-1}})).await;
    assert_eq!(
        response(&mut client, "bad").await["error"]["code"],
        "invalid_command"
    );
    for (kind, data, expected) in [
        ("trainer.setTargetPower", json!({"watts":5000}), json!(800)),
        (
            "trainer.setResistance",
            json!({"resistance":5.0}),
            json!(0.7),
        ),
        (
            "trainer.setSimulation",
            json!({"gradePercent":70.0,"windSpeedMps":99.0,"crr":0.4,"cw":5.0}),
            json!({"gradePercent":15.0,"windSpeedMps":20.0,"crr":0.02,"cw":1.0}),
        ),
    ] {
        send(
            &mut client,
            json!({"type":kind,"requestId":"limit","deviceId":"mock-trainer","data":data}),
        )
        .await;
        let reply = response(&mut client, "limit").await;
        assert_eq!(reply["success"], true);
        // Normalize f32 JSON rounding when checking clamped values.
        if expected.is_number() {
            assert!(
                (reply["data"]["applied"]["value"].as_f64().expect("value")
                    - expected.as_f64().expect("number"))
                .abs()
                    < 1e-6
            );
        } else {
            assert_eq!(reply["data"]["applied"]["value"]["gradePercent"], 15.0);
            assert_eq!(reply["data"]["applied"]["value"]["windSpeedMps"], 20.0);
        }
    }
    send(&mut client, json!({"type":"trainer.setTargetPower","requestId":"wrong","deviceId":"mock-controller","data":{"watts":200}})).await;
    assert_eq!(
        response(&mut client, "wrong").await["error"]["code"],
        "unsupported_operation"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn exclusive_control_is_released_and_load_reset_on_disconnect() {
    let daemon = Daemon::start(true).await;
    let mut owner = daemon.client().await;
    let mut observer = daemon.client().await;
    send(&mut owner, json!({"type":"trainer.setResistance","requestId":"load","deviceId":"mock-trainer","data":{"resistance":0.6}})).await;
    assert_eq!(response(&mut owner, "load").await["success"], true);
    send(&mut observer, json!({"type":"trainer.setTargetPower","requestId":"denied","deviceId":"mock-trainer","data":{"watts":400}})).await;
    assert_eq!(
        response(&mut observer, "denied").await["error"]["code"],
        "trainer_control_denied"
    );
    drop(owner); // Abrupt TCP loss, not just a graceful WebSocket close.
    timeout(Duration::from_secs(3), async {
        while daemon.state.status().await.websocket_clients != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cleanup");
    send(
        &mut observer,
        json!({"type":"subscribe","requestId":"sub","events":["telemetry"]}),
    )
    .await;
    assert_eq!(response(&mut observer, "sub").await["success"], true);
    assert_eq!(
        event(&mut observer, "telemetry").await["data"]["resistanceLevel"],
        0.0
    );
    send(&mut observer, json!({"type":"trainer.setTargetPower","requestId":"now","deviceId":"mock-trainer","data":{"watts":400}})).await;
    assert_eq!(response(&mut observer, "now").await["success"], true);
    daemon.stop().await;
}

#[tokio::test]
async fn subscriptions_filter_devices_and_can_be_replaced() {
    let daemon = Daemon::start(true).await;
    let mut client = daemon.client().await;
    send(&mut client, json!({"type":"subscribe","requestId":"sub","events":["telemetry","input"],"deviceIds":["mock-controller"]})).await;
    assert_eq!(response(&mut client, "sub").await["success"], true);
    assert!(
        timeout(Duration::from_millis(600), next_json(&mut client))
            .await
            .is_err(),
        "trainer telemetry must be filtered"
    );
    send(&mut client, json!({"type":"mock.input","requestId":"shift","deviceId":"mock-controller","data":{"input":"shift_down","state":"released"}})).await;
    assert_eq!(response(&mut client, "shift").await["success"], true);
    assert_eq!(
        event(&mut client, "input").await["data"]["input"],
        "shift_down"
    );
    send(
        &mut client,
        json!({"type":"subscribe","requestId":"off","events":[]}),
    )
    .await;
    assert_eq!(response(&mut client, "off").await["success"], true);
    send(&mut client, json!({"type":"mock.input","requestId":"hidden","deviceId":"mock-controller","data":{"input":"confirm","state":"pressed"}})).await;
    assert_eq!(response(&mut client, "hidden").await["success"], true);
    assert!(
        timeout(Duration::from_millis(350), next_json(&mut client))
            .await
            .is_err()
    );
    daemon.stop().await;
}

#[tokio::test]
async fn connection_lifecycle_and_mock_overrides() {
    let daemon = Daemon::start(true).await;
    let mut client = daemon.client().await;
    send(
        &mut client,
        json!({"type":"subscribe","requestId":"sub","events":["device","telemetry"]}),
    )
    .await;
    response(&mut client, "sub").await;
    send(
        &mut client,
        json!({"type":"device.disconnect","requestId":"disconnect","deviceId":"mock-trainer"}),
    )
    .await;
    assert_eq!(response(&mut client, "disconnect").await["success"], true);
    assert_eq!(
        event(&mut client, "device.disconnected").await["data"]["connected"],
        false
    );
    send(&mut client, json!({"type":"trainer.setTargetPower","requestId":"offline","deviceId":"mock-trainer","data":{"watts":200}})).await;
    assert_eq!(
        response(&mut client, "offline").await["error"]["code"],
        "device_disconnected"
    );
    send(
        &mut client,
        json!({"type":"device.connect","requestId":"connect","deviceId":"mock-trainer"}),
    )
    .await;
    assert_eq!(response(&mut client, "connect").await["success"], true);
    assert_eq!(
        event(&mut client, "device.connected").await["data"]["connected"],
        true
    );
    send(&mut client, json!({"type":"mock.setTelemetry","requestId":"override","deviceId":"mock-trainer","data":{"powerWatts":321,"cadenceRpm":92.0,"speedKph":35.0,"heartRateBpm":155}})).await;
    assert_eq!(response(&mut client, "override").await["success"], true);
    let sample = telemetry_matching(&mut client, |data| data["powerWatts"] == 321).await;
    assert_eq!(sample["data"]["powerWatts"], 321);
    assert_eq!(sample["data"]["cadenceRpm"], 92.0);
    assert_eq!(sample["data"]["speedKph"], 35.0);
    assert_eq!(sample["data"]["heartRateBpm"], 155);
    daemon.stop().await;
}

#[tokio::test]
async fn without_mock_has_no_fake_hardware() {
    let daemon = Daemon::start(false).await;
    assert!(daemon.state.devices().await.is_empty());
    let mut client = daemon.client().await;
    send(
        &mut client,
        json!({"type":"trainer.requestControl","requestId":"missing","deviceId":"mock-trainer"}),
    )
    .await;
    assert_eq!(
        response(&mut client, "missing").await["error"]["code"],
        "device_not_found"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn graceful_close_is_acknowledged_and_observer_exit_keeps_owner_control() {
    let daemon = Daemon::start(true).await;
    let mut owner = daemon.client().await;
    let mut observer = daemon.client().await;
    send(&mut owner, json!({"type":"trainer.setTargetPower","requestId":"erg","deviceId":"mock-trainer","data":{"watts":600}})).await;
    assert_eq!(response(&mut owner, "erg").await["success"], true);
    observer.close(None).await.expect("send close");
    timeout(Duration::from_secs(3), async {
        loop {
            match observer.next().await {
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Ping(_))) => {}
                other => panic!("Expected clean close reply, got {other:?}"),
            }
        }
    })
    .await
    .expect("close timeout");
    let mut another = daemon.client().await;
    send(
        &mut another,
        json!({"type":"trainer.requestControl","requestId":"denied","deviceId":"mock-trainer"}),
    )
    .await;
    assert_eq!(
        response(&mut another, "denied").await["error"]["code"],
        "trainer_control_denied"
    );
    daemon.stop().await;
}
