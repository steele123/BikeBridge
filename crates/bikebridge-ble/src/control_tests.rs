use super::*;

async fn controlled(
    limits: SafetyLimits,
) -> (
    Connections,
    Arc<Fake>,
    tokio::sync::broadcast::Receiver<Event>,
) {
    let bus = EventBus::default();
    let events = bus.subscribe();
    let connections = Connections::with_limits(bus, limits);
    let fake = Arc::new(Fake::default());
    fake.control_enabled.store(true, SeqCst);
    connections.register(&device(), fake.clone()).await;
    connections
        .set_connection("ble-fixture", true)
        .await
        .expect("connect");
    (connections, fake, events)
}
fn writes(fake: &Fake) -> Vec<Vec<u8>> {
    fake.writes
        .lock()
        .expect("mutex")
        .iter()
        .map(|(_, bytes)| bytes.clone())
        .collect()
}
async fn closed(fake: &Fake) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while fake.closes.load(SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("cleanup");
}
#[tokio::test(start_paused = true)]
async fn ownership_erg_simulation_and_owner_exit_safe_release() {
    let (connections, fake, _) = controlled(SafetyLimits::default()).await;
    let command = connections
        .execute(1, "ble-fixture", TrainerCommand::SetTargetPower(900))
        .await
        .expect("erg");
    assert_eq!(command, TrainerCommand::SetTargetPower(800));
    assert_eq!(writes(&fake), vec![vec![0], vec![5, 32, 3]]);
    assert_eq!(
        connections
            .execute(2, "ble-fixture", TrainerCommand::Stop)
            .await
            .expect_err("ownership")
            .code,
        ErrorCode::TrainerControlDenied
    );
    assert!(
        connections
            .set_connection_for(0, "ble-fixture", false)
            .await
            .is_err()
    );
    assert_eq!(writes(&fake).len(), 2);
    connections.release_client(2).await;
    assert_eq!(fake.closes.load(SeqCst), 0);
    connections
        .execute(
            1,
            "ble-fixture",
            TrainerCommand::SetSimulation(bikebridge_core::TrainerSimulation {
                grade_percent: 99.0,
                wind_speed_mps: -1.0,
                crr: 0.004,
                cw: 0.51,
            }),
        )
        .await
        .expect("simulation");
    assert_eq!(writes(&fake)[2], vec![17, 24, 252, 220, 5, 40, 51]);
    connections.release_client(1).await;
    closed(&fake).await;
    assert_eq!(&writes(&fake)[3..], &[vec![8, 1], vec![1]]);
    connections.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn reset_releases_ownership_and_stop_reacquires_permission_before_next_target() {
    let (connections, fake, _) = controlled(SafetyLimits::default()).await;
    fake.emit_status.store(true, SeqCst);
    connections
        .execute(1, "ble-fixture", TrainerCommand::RequestControl)
        .await
        .expect("control");
    connections
        .execute(1, "ble-fixture", TrainerCommand::Stop)
        .await
        .expect("stop/reset");
    connections
        .execute(1, "ble-fixture", TrainerCommand::SetTargetPower(200))
        .await
        .expect("new control");
    connections
        .execute(1, "ble-fixture", TrainerCommand::Reset)
        .await
        .expect("reset");
    connections
        .execute(2, "ble-fixture", TrainerCommand::RequestControl)
        .await
        .expect("new owner");
    assert_eq!(
        writes(&fake),
        vec![
            vec![0],
            vec![8, 1],
            vec![1],
            vec![0],
            vec![5, 200, 0],
            vec![1],
            vec![0]
        ]
    );
    connections.shutdown().await;
    assert_eq!(&writes(&fake)[7..], &[vec![8, 1], vec![1]]);
}
#[tokio::test(start_paused = true)]
async fn timeout_wrong_opcode_denied_and_write_failure_never_retry_an_uncertain_target() {
    for (mode, code) in [
        (1, ErrorCode::Timeout),
        (2, ErrorCode::InvalidDeviceData),
        (3, ErrorCode::TrainerControlDenied),
        (5, ErrorCode::ConnectionFailed),
    ] {
        let (connections, fake, _) = controlled(SafetyLimits::default()).await;
        connections
            .execute(1, "ble-fixture", TrainerCommand::RequestControl)
            .await
            .expect("control");
        fake.reply_mode.store(mode, SeqCst);
        assert_eq!(
            connections
                .execute(1, "ble-fixture", TrainerCommand::SetTargetPower(200))
                .await
                .expect_err("failed")
                .code,
            code
        );
        closed(&fake).await;
        assert_eq!(
            writes(&fake),
            vec![vec![0], vec![5, 200, 0]],
            "must not overlap an uncertain transaction or reclaim denied control"
        );
        fake.reply_mode.store(0, SeqCst);
        connections
            .set_connection("ble-fixture", true)
            .await
            .expect("reconnect");
        connections
            .execute(2, "ble-fixture", TrainerCommand::SetTargetPower(100))
            .await
            .expect("new lease");
        assert_eq!(&writes(&fake)[2..], &[vec![0], vec![5, 100, 0]]);
        connections.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn explicit_failed_response_allows_best_effort_stop_reset() {
    let (connections, fake, _) = controlled(SafetyLimits::default()).await;
    connections
        .execute(1, "ble-fixture", TrainerCommand::RequestControl)
        .await
        .expect("control");
    fake.reply_mode.store(4, SeqCst);
    assert_eq!(
        connections
            .execute(1, "ble-fixture", TrainerCommand::Start)
            .await
            .expect_err("failure")
            .code,
        ErrorCode::TrainerControlFailed
    );
    closed(&fake).await;
    assert_eq!(writes(&fake), vec![vec![0], vec![7], vec![8, 1], vec![1]]);
    connections.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn smoothing_limits_confirmed_increases_and_another_mode_cancels_ramp() {
    let limits = SafetyLimits {
        max_resistance_change_per_second: 0.2,
        ..Default::default()
    };
    let (connections, fake, _) = controlled(limits).await;
    let applied = connections
        .execute(1, "ble-fixture", TrainerCommand::SetResistance(1.0))
        .await
        .expect("ramp");
    assert_eq!(applied, TrainerCommand::SetResistance(0.7));
    for _ in 0..20 {
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
    }
    let records = fake.writes.lock().expect("mutex").clone();
    let mut previous = None;
    for (time, bytes) in records.iter().filter(|(_, bytes)| bytes[0] == 4) {
        let level = f32::from(i16::from_le_bytes([bytes[1], bytes[2]])) / 1000.0;
        assert!(level <= 0.7);
        if let Some((last_time, last_level)) = previous {
            let elapsed = time.duration_since(last_time).as_secs_f32();
            assert!(level - last_level <= elapsed * 0.2 + 0.00001);
        }
        previous = Some((*time, level));
    }
    assert_eq!(previous.expect("steps").1, 0.7);
    connections
        .execute(1, "ble-fixture", TrainerCommand::SetResistance(0.1))
        .await
        .expect("decrease");
    tokio::time::advance(Duration::from_millis(250)).await;
    tokio::task::yield_now().await;
    connections
        .execute(1, "ble-fixture", TrainerCommand::SetTargetPower(200))
        .await
        .expect("switch");
    let count = writes(&fake).len();
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert_eq!(writes(&fake).len(), count);
    connections.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn control_permission_lost_cancels_ramp_without_fighting_the_trainer() {
    let (connections, fake, _) = controlled(SafetyLimits::default()).await;
    connections
        .execute(1, "ble-fixture", TrainerCommand::SetResistance(0.5))
        .await
        .expect("ramp");
    let sender = fake
        .control_sender
        .lock()
        .expect("mutex")
        .as_ref()
        .expect("control")
        .clone();
    sender
        .send(ControlEvent::Status(vec![255]))
        .await
        .expect("lost control");
    closed(&fake).await;
    assert_eq!(writes(&fake), vec![vec![0], vec![4, 0, 0]]);
    connections.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn configured_reconnect_is_bounded_and_never_replays_control() {
    let (connections, fake, mut events) = controlled(SafetyLimits {
        auto_reconnect: true,
        ..Default::default()
    })
    .await;
    connections
        .execute(1, "ble-fixture", TrainerCommand::RequestControl)
        .await
        .expect("control");
    fake.connected.store(false, SeqCst);
    let retry = loop {
        let event = receive(&mut events).await;
        if matches!(event, Event::DeviceReconnecting { .. }) {
            break event;
        }
    };
    assert!(matches!(
        retry,
        Event::DeviceReconnecting {
            attempt: 1,
            delay_seconds: 1,
            ..
        }
    ));
    let before = writes(&fake).len();
    loop {
        if matches!(receive(&mut events).await, Event::DeviceConnected { .. }) {
            break;
        }
    }
    assert_eq!(fake.opens.load(SeqCst), 2);
    assert_eq!(writes(&fake).len(), before);
    connections
        .execute(2, "ble-fixture", TrainerCommand::RequestControl)
        .await
        .expect("fresh control");
    connections
        .set_connection_for(2, "ble-fixture", false)
        .await
        .expect("intentional disconnect");
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert_eq!(fake.opens.load(SeqCst), 2);
    connections.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn reconnect_attempts_stop_after_five_failures() {
    let (connections, fake, mut events) = controlled(SafetyLimits {
        auto_reconnect: true,
        ..Default::default()
    })
    .await;
    fake.open_mode.store(1, SeqCst);
    fake.connected.store(false, SeqCst);
    tokio::time::timeout(Duration::from_secs(90), async {
        while fake.opens.load(SeqCst) < 6 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("bounded retries");
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::task::yield_now().await;
    assert_eq!(fake.opens.load(SeqCst), 6);
    let mut delays = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let Event::DeviceReconnecting {
            attempt,
            delay_seconds,
            ..
        } = event
        {
            delays.push((attempt, delay_seconds));
        }
    }
    assert_eq!(delays, vec![(1, 1), (2, 2), (3, 5), (4, 10), (5, 30)]);
    connections.shutdown().await;
}
