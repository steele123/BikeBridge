use super::*;
use bikebridge_core::{CommandOutcome, DeviceKind, EventBus, TrainerCommand, TrainerTelemetry};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "bikebridge-trace-{}-{}.biketrace",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn device(connected: bool) -> DeviceInfo {
    DeviceInfo {
        id: "trainer".into(),
        name: "Fixture".into(),
        kind: DeviceKind::Trainer,
        transport: "mock".into(),
        connected,
        signal_strength: None,
        capabilities: vec![],
    }
}
fn fixture() -> String {
    let events = [
        Event::Telemetry {
            device_id: "trainer".into(),
            data: TrainerTelemetry {
                power_watts: Some(240),
                cadence_rpm: Some(90.0),
                heart_rate_bpm: Some(140),
                timestamp_ms: 1234,
                ..Default::default()
            },
        },
        Event::CommandStarted {
            command_id: 1,
            session_id: 1,
            device_id: "trainer".into(),
            command: TrainerCommand::SetResistance(0.4),
        },
        Event::CommandFinished {
            command_id: 1,
            device_id: "trainer".into(),
            outcome: CommandOutcome::Applied {
                command: TrainerCommand::SetResistance(0.4),
            },
        },
        Event::DeviceDisconnected {
            data: device(false),
        },
    ];
    let mut lines = vec![Line::Header {
        data: Header {
            format: "bikebridge.trace".into(),
            version: 1,
            protocol_version: 1,
            bike_bridge_version: "test".into(),
            created_at_ms: 1000,
            devices: vec![device(true)],
        },
    }];
    for (sequence, event) in events.into_iter().enumerate() {
        lines.push(Line::Event {
            data: Record {
                sequence: sequence as u64,
                offset_us: if sequence == 0 { 0 } else { 1_000_000 },
                event,
            },
        });
    }
    lines.push(Line::End {
        events: 4,
        duration_us: 2_000_000,
    });
    lines
        .iter()
        .map(|line| format!("{}\n", serde_json::to_string(line).expect("JSON")))
        .collect()
}
#[test]
fn record_round_trip_order_timestamps_and_exclusive_creation() {
    let path = Temp::new();
    let bus = EventBus::default();
    let recorder = Recorder::start(&path.0, bus.clone(), vec![device(true)]).expect("record");
    let expected = Trace::read(fixture().as_bytes()).expect("fixture");
    for record in expected.records() {
        bus.publish(record.event.clone());
    }
    let summary = recorder.finish().expect("finish");
    assert_eq!(summary.events, 4);
    let trace = Trace::load(&path.0).expect("load complete trace");
    assert_eq!(trace.header().devices, vec![device(true)]);
    assert_eq!(
        trace.records().iter().map(|r| &r.event).collect::<Vec<_>>(),
        expected
            .records()
            .iter()
            .map(|r| &r.event)
            .collect::<Vec<_>>()
    );
    assert!(
        trace
            .records()
            .windows(2)
            .all(|r| r[0].offset_us <= r[1].offset_us)
    );
    let before = std::fs::read(&path.0).expect("bytes");
    assert!(Recorder::start(&path.0, bus, vec![]).is_err());
    assert_eq!(std::fs::read(&path.0).expect("bytes"), before);
}
#[test]
fn reject_truncation_version_sequence_time_footer_and_limits() {
    let valid = fixture();
    let lines: Vec<_> = valid.lines().collect();
    let bad = [
        lines[..lines.len() - 1].join("\n") + "\n",
        valid.trim_end().to_string(),
        valid.replace("\"version\":1", "\"version\":2"),
        valid.replace("\"sequence\":1", "\"sequence\":9"),
        valid.replacen("\"offsetUs\":1000000", "\"offsetUs\":2000000", 1),
        valid.replace("\"events\":4", "\"events\":3"),
        valid.clone() + "{}\n",
        format!("{}\n{}", lines[0], valid),
        "x".repeat(MAX_LINE + 1),
    ];
    for invalid in bad {
        assert!(
            Trace::read(invalid.as_bytes()).is_err(),
            "must reject {invalid:.150}"
        );
    }
}
#[test]
fn replay_preserves_values_equal_time_order_pause_speed_eof_and_restart() {
    let trace = Trace::read(fixture().as_bytes()).expect("trace");
    let expected: Vec<_> = trace.records().iter().map(|r| r.event.clone()).collect();
    let mut replay = Playback::new(trace, 2.0).expect("playback");
    assert!(replay.advance(Duration::from_secs(10)).is_empty());
    replay.start();
    assert_eq!(replay.advance(Duration::ZERO), expected[..1]);
    assert!(replay.advance(Duration::from_millis(200)).is_empty());
    replay.pause();
    assert!(replay.advance(Duration::from_secs(5)).is_empty());
    assert_eq!(replay.status().position_us, 400_000);
    replay.start();
    assert_eq!(replay.advance(Duration::from_millis(300)), expected[1..]);
    assert!(!replay.devices()[0].connected);
    assert!(!replay.status().finished);
    replay.advance(Duration::from_millis(500));
    assert!(replay.status().finished);
    assert!(replay.advance(Duration::from_secs(9)).is_empty());
    assert!(matches!(replay.restart(), Event::ReplayReset { .. }));
    assert!(replay.devices()[0].connected);
    assert_eq!(replay.advance(Duration::from_secs(1)), expected);
    for speed in [0.0, 17.0, f64::NAN, f64::INFINITY] {
        assert!(Playback::new(Trace::read(fixture().as_bytes()).expect("trace"), speed).is_err());
    }
}
#[test]
fn concurrent_publication_has_identical_recording_and_subscriber_order() {
    let path = Temp::new();
    let bus = EventBus::default();
    let recorder = Recorder::start(&path.0, bus.clone(), vec![]).expect("start");
    let mut receiver = bus.subscribe();
    std::thread::scope(|scope| {
        for thread in 0..4 {
            let bus = bus.clone();
            scope.spawn(move || {
                for n in 0..40 {
                    bus.publish(Event::SessionDisconnected {
                        session_id: thread * 40 + n,
                    });
                }
            });
        }
    });
    recorder.finish().expect("finish");
    let trace = Trace::load(&path.0).expect("trace");
    for record in trace.records() {
        assert_eq!(record.event, receiver.try_recv().expect("event"));
    }
    assert_eq!(trace.records().len(), 160);
}
