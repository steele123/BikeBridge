// Included from mock_api.rs to reuse its real loopback daemon harness.
use super::*;
use std::sync::{Arc, atomic::Ordering::SeqCst};

#[tokio::test]
async fn websocket_control_capabilities_clamping_ownership_and_responsive_telemetry() {
    let transport = Arc::new(TelemetryFixture {
        control: true,
        ..Default::default()
    });
    let (daemon, _, id) = telemetry_daemon(transport.clone()).await;
    let host = daemon.address.to_string();
    let (_, device) = daemon
        .request("POST", &format!("/api/devices/{id}/connect"), &host, "")
        .await;
    assert_eq!(
        device["capabilities"],
        json!([
            "speed",
            "cadence",
            "power",
            "resistance_control",
            "erg_control",
            "simulation_control"
        ])
    );
    let mut owner = daemon.client().await;
    let mut observer = daemon.client().await;
    send(
        &mut owner,
        json!({"type":"trainer.requestControl","deviceId":id,"requestId":"own"}),
    )
    .await;
    assert_eq!(response(&mut owner, "own").await["success"], true);
    send(&mut observer,json!({"type":"trainer.setTargetPower","deviceId":id,"requestId":"denied","data":{"watts":250}})).await;
    assert_eq!(
        response(&mut observer, "denied").await["error"]["code"],
        "trainer_control_denied"
    );
    assert_eq!(
        daemon
            .request("POST", &format!("/api/devices/{id}/disconnect"), &host, "")
            .await
            .0,
        409
    );
    assert_eq!(
        transport.writes.lock().expect("mutex").as_slice(),
        &[vec![0]]
    );
    // Delay the actual trainer indication. The owner socket must keep processing subscriptions and telemetry.
    transport.hold_response.store(true, SeqCst);
    send(&mut owner,json!({"type":"trainer.setTargetPower","deviceId":id,"requestId":"erg","data":{"watts":900}})).await;
    send(
        &mut owner,
        json!({"type":"subscribe","requestId":"sub","events":["telemetry"]}),
    )
    .await;
    assert_eq!(response(&mut owner, "sub").await["success"], true);
    transport.packet(&[68, 0, 184, 11, 180, 0, 250, 0]).await;
    assert_eq!(
        event(&mut owner, "telemetry").await["data"]["powerWatts"],
        250
    );
    timeout(Duration::from_secs(2), async {
        while transport.writes.lock().expect("mutex").len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("write");
    assert_eq!(transport.writes.lock().expect("mutex")[1], vec![5, 32, 3]);
    transport.acknowledge(5).await;
    transport.hold_response.store(false, SeqCst);
    assert_eq!(
        response(&mut owner, "erg").await["data"]["applied"]["value"],
        800
    );
    send(&mut owner,json!({"type":"trainer.setResistance","deviceId":id,"requestId":"ramp","data":{"resistance":10.0}})).await;
    assert_eq!(
        response(&mut owner, "ramp").await["data"]["applied"]["value"],
        json!(0.7_f32)
    );
    assert_eq!(transport.writes.lock().expect("mutex")[2], vec![4, 0, 0]);
    drop(owner);
    timeout(Duration::from_secs(3), async {
        while transport.closed.load(SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owner cleanup");
    let writes = transport.writes.lock().expect("mutex").clone();
    assert_eq!(&writes[writes.len() - 2..], &[vec![8, 1], vec![1]]);
    assert_eq!(daemon.state.status().await.connected_devices, 0);
    daemon.stop().await;
}

#[tokio::test]
async fn client_loss_during_unacknowledged_command_disconnects_without_another_write() {
    let transport = Arc::new(TelemetryFixture {
        control: true,
        ..Default::default()
    });
    let (daemon, _, id) = telemetry_daemon(transport.clone()).await;
    let mut audit = daemon.state.events.subscribe();
    daemon
        .request(
            "POST",
            &format!("/api/devices/{id}/connect"),
            &daemon.address.to_string(),
            "",
        )
        .await;
    let mut owner = daemon.client().await;
    send(
        &mut owner,
        json!({"type":"trainer.requestControl","deviceId":id,"requestId":"own"}),
    )
    .await;
    assert_eq!(response(&mut owner, "own").await["success"], true);
    transport.hold_response.store(true, SeqCst);
    send(&mut owner,json!({"type":"trainer.setTargetPower","deviceId":id,"requestId":"pending","data":{"watts":250}})).await;
    timeout(Duration::from_secs(2), async {
        while transport.writes.lock().expect("mutex").len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("write");
    drop(owner);
    timeout(Duration::from_secs(2), async {
        while transport.closed.load(SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("immediate cancellation cleanup");
    assert_eq!(
        transport.writes.lock().expect("mutex").as_slice(),
        &[vec![0], vec![5, 250, 0]]
    );
    let mut cancelled = false;
    while let Ok(event) = audit.try_recv() {
        if matches!(
            event,
            bikebridge_core::Event::CommandFinished {
                outcome: bikebridge_core::CommandOutcome::Cancelled,
                ..
            }
        ) {
            cancelled = true;
        }
    }
    assert!(
        cancelled,
        "in-flight client loss must be recorded as cancelled, never successful"
    );
    daemon.stop().await;
}
