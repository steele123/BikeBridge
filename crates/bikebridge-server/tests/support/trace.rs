use super::*;
use bikebridge_core::{CommandOutcome, Event, TrainerCommand};
use bikebridge_trace::{Recorder, Trace};

#[tokio::test]
async fn record_live_api_and_replay_identical_events_without_hardware() {
    let path =
        std::env::temp_dir().join(format!("bikebridge-api-{}.biketrace", std::process::id()));
    let state = AppState::new(true, SafetyLimits::default()).expect("state");
    let recorder =
        Recorder::start(&path, state.events.clone(), state.devices().await).expect("recorder");
    let daemon = Daemon::with_state(state).await;
    let mut owner = daemon.client().await;
    send(
        &mut owner,
        json!({"type":"subscribe","requestId":"sub","events":["telemetry"]}),
    )
    .await;
    assert_eq!(response(&mut owner, "sub").await["success"], true);
    send(&mut owner,json!({"type":"trainer.setResistance","requestId":"r","deviceId":"mock-trainer","data":{"resistance":0.9}})).await;
    assert_eq!(
        response(&mut owner, "r").await["data"]["applied"]["value"],
        json!(0.7_f32)
    );
    send(&mut owner,json!({"type":"mock.setTelemetry","requestId":"t","deviceId":"mock-trainer","data":{"powerWatts":321,"cadenceRpm":93.5,"heartRateBpm":147}})).await;
    assert_eq!(response(&mut owner, "t").await["success"], true);
    telemetry_matching(&mut owner, |t| {
        t["powerWatts"] == 321 && t["heartRateBpm"] == 147
    })
    .await;
    send(&mut owner,json!({"type":"mock.input","requestId":"shift","deviceId":"mock-controller","data":{"input":"shift_up","state":"pressed"}})).await;
    assert_eq!(response(&mut owner, "shift").await["success"], true);
    let mut rival = daemon.client().await;
    send(&mut rival,json!({"type":"trainer.setTargetPower","requestId":"denied","deviceId":"mock-trainer","data":{"watts":150}})).await;
    assert_eq!(
        response(&mut rival, "denied").await["error"]["code"],
        "trainer_control_denied"
    );
    send(
        &mut owner,
        json!({"type":"device.disconnect","requestId":"bye","deviceId":"mock-trainer"}),
    )
    .await;
    assert_eq!(response(&mut owner, "bye").await["success"], true);
    daemon.stop().await;
    let summary = tokio::task::spawn_blocking(move || recorder.finish())
        .await
        .expect("join")
        .expect("finish");
    let trace = Trace::load(&path).expect("valid capture");
    std::fs::remove_file(&path).expect("remove fixture");
    let expected: Vec<_> = trace
        .records()
        .iter()
        .map(|r| {
            serde_json::from_str::<Value>(&serde_json::to_string(&r.event).expect("encode"))
                .expect("JSON")
        })
        .collect();
    assert_eq!(summary.events, expected.len() as u64);
    assert!(
        trace
            .records()
            .iter()
            .any(|r| matches!(r.event, Event::Input { .. }))
    );
    assert!(trace.records().iter().any(|r|matches!(r.event,Event::CommandStarted {command:TrainerCommand::SetResistance(value),..} if value==0.9)));
    assert!(trace.records().iter().any(|r|matches!(r.event,Event::CommandFinished {outcome:CommandOutcome::Applied {command:TrainerCommand::SetResistance(value)},..} if value==0.7)));
    assert!(trace.records().iter().any(|r|matches!(&r.event,Event::CommandFinished {outcome:CommandOutcome::Failed {error},..} if error.code==bikebridge_core::ErrorCode::TrainerControlDenied)));
    assert!(
        trace
            .records()
            .iter()
            .any(|r| matches!(r.event, Event::SessionDisconnected { .. }))
    );

    let replay = Daemon::with_state(AppState::replay(trace, 16.0).expect("replay")).await;
    let mut viewer = replay.client().await;
    send(&mut viewer,json!({"type":"subscribe","requestId":"all","events":["telemetry","input","device","scan","error","command","session","replay"]})).await;
    assert_eq!(response(&mut viewer, "all").await["success"], true);
    assert!(
        timeout(Duration::from_millis(30), next_json(&mut viewer))
            .await
            .is_err(),
        "paused timeline must not emit"
    );
    send(&mut viewer,json!({"type":"trainer.setTargetPower","requestId":"never-write","deviceId":"mock-trainer","data":{"watts":900}})).await;
    assert_eq!(
        response(&mut viewer, "never-write").await["error"]["code"],
        "unsupported_operation"
    );
    let host = replay.address.to_string();
    assert_eq!(
        replay
            .request("POST", "/api/replay/start", &host, "")
            .await
            .0,
        200
    );
    for expected in &expected {
        assert_eq!(&next_json(&mut viewer).await, expected);
    }
    timeout(Duration::from_secs(2), async {
        while !replay.state.replay_status().await.expect("replay").finished {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("EOF");
    let status = replay.http("/api/status", &host, "").await.1;
    assert_eq!(status["bluetoothEnabled"], false);
    assert_eq!(status["mockMode"], false);
    assert_eq!(status["replay"]["finished"], true);
    let devices = replay.http("/api/devices", &host, "").await.1;
    assert_eq!(
        devices
            .as_array()
            .expect("devices")
            .iter()
            .find(|d| d["id"] == "mock-trainer")
            .expect("trainer")["connected"],
        false
    );
    assert_eq!(
        replay
            .request("POST", "/api/replay/restart", &host, "")
            .await
            .0,
        200
    );
    assert_eq!(next_json(&mut viewer).await["type"], "replay.reset");
    for expected in &expected {
        assert_eq!(&next_json(&mut viewer).await, expected);
    }
    replay.stop().await;
}
