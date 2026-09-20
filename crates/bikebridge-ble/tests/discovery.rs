//! Injected discovery tests; no platform manager or radio is initialized.
use bikebridge_ble::{Advertisement, BackendAdapter, DiscoveryBackend, Scanner, classification::*};
use bikebridge_core::{AdapterState, BridgeError, ErrorCode, Event, EventBus, Result};
use std::{future::pending, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Default)]
struct State {
    selected_names: Vec<String>,
    adapters: Vec<BackendAdapter>,
    advertisements: Vec<Advertisement>,
    starts: Vec<String>,
    stops: usize,
    start_fails: bool,
    stop_fails: bool,
    poll_fails: bool,
    hang_start: bool,
}
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

fn failure() -> BridgeError {
    BridgeError::new(ErrorCode::ScanFailed, "Injected failure.")
}
impl DiscoveryBackend for Fake {
    fn select_device(&mut self, _: &str, key: &str) -> Result<()> {
        let mut state = self.0.try_lock().expect("uncontended fixture");
        state
            .advertisements
            .iter_mut()
            .find(|ad| ad.key == key)
            .expect("known key")
            .manually_selected = true;
        Ok(())
    }
    fn select_name(&mut self, name: String) -> Result<()> {
        self.0
            .try_lock()
            .expect("uncontended fixture")
            .selected_names
            .push(name);
        Ok(())
    }
    async fn adapters(&mut self) -> Result<Vec<BackendAdapter>> {
        Ok(self.0.lock().await.adapters.clone())
    }
    async fn start_scan(&mut self, key: &str) -> Result<()> {
        let mut state = self.0.lock().await;
        state.starts.push(key.to_owned());
        if state.start_fails {
            return Err(failure());
        }
        let hang = state.hang_start;
        drop(state);
        if hang {
            pending::<()>().await;
        }
        Ok(())
    }
    async fn stop_scan(&mut self, _: &str) -> Result<()> {
        let mut state = self.0.lock().await;
        state.stops += 1;
        if state.stop_fails {
            Err(failure())
        } else {
            Ok(())
        }
    }
    async fn advertisements(&mut self, _: &str) -> Result<Vec<Advertisement>> {
        let state = self.0.lock().await;
        if state.poll_fails {
            Err(failure())
        } else {
            Ok(state.advertisements.clone())
        }
    }
}
fn adapter(key: &str, state: AdapterState) -> BackendAdapter {
    BackendAdapter {
        key: key.into(),
        state,
    }
}
fn advertisement() -> Advertisement {
    Advertisement {
        click_v2: None,
        manually_selected: false,
        key: "private-peripheral-address".into(),
        name: Some("Test Trainer".into()),
        rssi: Some(-52),
        services: vec![FITNESS_MACHINE],
    }
}
async fn powered() -> Fake {
    let fake = Fake::default();
    fake.0.lock().await.adapters = vec![
        adapter("off", AdapterState::PoweredOff),
        adapter("on", AdapterState::PoweredOn),
    ];
    fake
}
async fn next_device(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
) -> bikebridge_core::DeviceInfo {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Event::DeviceDiscovered { data } = events.recv().await.expect("event") {
                return data;
            }
        }
    })
    .await
    .expect("discovery deadline")
}

#[tokio::test(start_paused = true)]
async fn default_adapter_start_stop_dedup_and_identity_across_rescans() {
    let fake = powered().await;
    fake.0.lock().await.advertisements.push(advertisement());
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let scanner = Scanner::spawn(fake.clone(), bus, None).await;
    assert!(!scanner.snapshot().await.scan.scanning);
    assert!(scanner.snapshot().await.adapters[1].is_default);
    assert!(scanner.start().await.expect("start").scanning);
    scanner.start().await.expect("idempotent");
    assert_eq!(fake.0.lock().await.starts, vec!["on"]);
    let device = next_device(&mut events).await;
    assert!(device.id.starts_with("ble-"));
    assert_eq!(device.signal_strength, Some(-52));
    assert!(device.capabilities.is_empty());
    assert!(!device.connected);
    assert!(
        !serde_json::to_string(&device)
            .expect("json")
            .contains("private-peripheral")
    );
    scanner.stop().await.expect("stop");
    scanner.stop().await.expect("idempotent stop");
    assert_eq!(fake.0.lock().await.stops, 1);
    scanner.start().await.expect("restart");
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(scanner.snapshot().await.devices[0].id, device.id);
    assert_eq!(scanner.snapshot().await.devices.len(), 1);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, Event::DeviceDiscovered { .. }));
    }
    scanner.shutdown().await;
    assert_eq!(fake.0.lock().await.stops, 2);
    assert!(scanner.start().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn absent_off_and_out_of_range_adapters_are_distinct_from_empty_scan() {
    let fake = Fake::default();
    let scanner = Scanner::spawn(fake.clone(), EventBus::default(), None).await;
    assert_eq!(
        scanner.start().await.expect_err("no adapter").code,
        ErrorCode::AdapterNotFound
    );
    assert!(!scanner.snapshot().await.available);
    fake.0
        .lock()
        .await
        .adapters
        .push(adapter("one", AdapterState::PoweredOff));
    assert_eq!(
        scanner.start().await.expect_err("off").code,
        ErrorCode::BluetoothUnavailable
    );
    fake.0.lock().await.adapters[0].state = AdapterState::PoweredOn;
    scanner.start().await.expect("recover");
    let snapshot = scanner.snapshot().await;
    assert!(snapshot.available && snapshot.scan.scanning);
    assert!(snapshot.scan.last_error.is_none());
    assert!(snapshot.devices.is_empty());
    scanner.shutdown().await;
    let scanner = Scanner::spawn(fake, EventBus::default(), Some(99)).await;
    assert_eq!(
        scanner.start().await.expect_err("bad selection").code,
        ErrorCode::AdapterNotFound
    );
    scanner.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn timeout_after_partial_start_attempts_stop() {
    let fake = powered().await;
    fake.0.lock().await.hang_start = true;
    let scanner = Scanner::spawn(fake.clone(), EventBus::default(), None).await;
    assert_eq!(
        scanner.start().await.expect_err("timeout").code,
        ErrorCode::Timeout
    );
    assert!(!scanner.snapshot().await.scan.scanning);
    assert_eq!(fake.0.lock().await.stops, 1);
    scanner.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn failed_start_and_failed_stop_remain_observable_and_retryable() {
    let fake = powered().await;
    {
        let mut state = fake.0.lock().await;
        state.start_fails = true;
        state.stop_fails = true;
    }
    let scanner = Scanner::spawn(fake.clone(), EventBus::default(), None).await;
    assert_eq!(
        scanner.start().await.expect_err("start failure").code,
        ErrorCode::ScanFailed
    );
    assert!(
        scanner.snapshot().await.scan.scanning,
        "uncertain activity must not claim a clean stop"
    );
    assert!(scanner.snapshot().await.scan.last_error.is_some());
    assert!(scanner.stop().await.is_err());
    {
        let mut state = fake.0.lock().await;
        state.start_fails = false;
        state.stop_fails = false;
    }
    scanner.stop().await.expect("retry stop");
    scanner.start().await.expect("retry start");
    assert!(scanner.snapshot().await.scan.last_error.is_none());
    scanner.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn poll_failure_stops_scan_and_emits_error_without_spinning() {
    let fake = powered().await;
    fake.0.lock().await.poll_fails = true;
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let scanner = Scanner::spawn(fake.clone(), bus, None).await;
    scanner.start().await.expect("start");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Event::Error { data } = events.recv().await.expect("event") {
                assert_eq!(data.code, ErrorCode::ScanFailed);
                break;
            }
        }
    })
    .await
    .expect("error deadline");
    assert!(!scanner.snapshot().await.scan.scanning);
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(fake.0.lock().await.starts.len(), 1);
    assert_eq!(fake.0.lock().await.stops, 1);
    scanner.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn manual_selection_includes_only_opted_in_unknown_devices() {
    let fake = powered().await;
    let mut selected = advertisement();
    selected.name = Some("Steele’s Bike".into());
    selected.services.clear();
    selected.manually_selected = true;
    let mut unrelated = selected.clone();
    unrelated.key = "unrelated-device".into();
    unrelated.name = Some("TV".into());
    unrelated.manually_selected = false;
    fake.0.lock().await.advertisements = vec![selected, unrelated];
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let scanner = Scanner::spawn(fake, bus, None).await;
    scanner.start().await.expect("scan");
    let device = next_device(&mut events).await;
    assert_eq!(device.name, "Steele’s Bike");
    assert_eq!(device.kind, bikebridge_core::DeviceKind::Unknown);
    assert!(!device.connected);
    assert!(device.capabilities.is_empty());
    assert_eq!(scanner.snapshot().await.devices.len(), 1);
    scanner.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn selecting_name_restarts_scan_and_rejects_invalid_names_before_mutation() {
    let fake = powered().await;
    let scanner = Scanner::spawn(fake.clone(), EventBus::default(), None).await;
    scanner.start().await.expect("start");
    for name in [
        "".to_owned(),
        "  ".to_owned(),
        "a".repeat(129),
        "bad\nname".to_owned(),
    ] {
        assert_eq!(
            scanner
                .select_name(name)
                .await
                .expect_err("invalid name")
                .code,
            ErrorCode::InvalidValue
        );
    }
    assert!(fake.0.lock().await.selected_names.is_empty());
    assert!(
        scanner
            .select_name("Steele's Bike".into())
            .await
            .expect("select")
            .scanning
    );
    {
        let state = fake.0.lock().await;
        assert_eq!(state.selected_names, vec!["Steele's Bike"]);
        assert_eq!(state.starts.len(), 2);
        assert_eq!(state.stops, 1);
    }
    scanner.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn nearby_names_are_separate_and_selection_uses_identity_not_duplicate_name() {
    let fake = powered().await;
    let mut first = advertisement();
    first.services.clear();
    first.name = Some("Same name".into());
    let mut second = first.clone();
    second.key = "second-private-address".into();
    let mut unnamed = first.clone();
    unnamed.key = "third-private-address".into();
    unnamed.name = None;
    fake.0.lock().await.advertisements = vec![first, second, unnamed];
    let scanner = Scanner::spawn(fake.clone(), EventBus::default(), None).await;
    scanner.start().await.expect("scan");
    let snapshot = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let snapshot = scanner.snapshot().await;
            if snapshot.nearby.len() == 3 {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("nearby results");
    assert!(snapshot.devices.is_empty());
    assert_eq!(
        snapshot
            .nearby
            .iter()
            .filter(|d| d.name.as_deref() == Some("Same name"))
            .count(),
        2
    );
    assert!(snapshot.nearby.iter().any(|d| d.name.is_none()));
    assert!(snapshot.nearby.iter().all(|d| d.device_id.is_none()));
    let json = serde_json::to_string(&snapshot.nearby).expect("JSON");
    assert!(!json.contains("private"));
    let chosen = snapshot.nearby[0].id.clone();
    scanner.select_device(chosen.clone()).await.expect("select");
    let selected = scanner.snapshot().await;
    assert_eq!(selected.devices.len(), 1);
    assert!(!selected.devices[0].connected);
    assert!(selected.devices[0].capabilities.is_empty());
    assert_eq!(
        selected
            .nearby
            .iter()
            .filter(|d| d.device_id.is_some())
            .count(),
        1
    );
    assert_eq!(
        selected
            .nearby
            .iter()
            .find(|d| d.id == chosen)
            .expect("same identity")
            .device_id
            .as_deref(),
        Some(selected.devices[0].id.as_str())
    );
    assert_eq!(
        scanner
            .select_device("missing".into())
            .await
            .expect_err("bad ID")
            .code,
        ErrorCode::DeviceNotFound
    );
    assert!(scanner.snapshot().await.scan.scanning);
    assert!(scanner.snapshot().await.scan.last_error.is_none());
    scanner.stop().await.expect("stop");
    assert_eq!(scanner.snapshot().await.nearby, selected.nearby);
    scanner
        .select_device(chosen)
        .await
        .expect("restart and select idempotently");
    assert_eq!(scanner.snapshot().await.devices.len(), 1);
    scanner.shutdown().await;
}
