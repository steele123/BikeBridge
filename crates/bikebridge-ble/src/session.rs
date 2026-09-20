use crate::{
    control::{decode_response, invalid, unsupported},
    ftms::RecordAssembler,
    transport::{ControlEvent, FtmsSession, FtmsTransport, TelemetryFormat},
};
use bikebridge_core::{
    BridgeError, ErrorCode, Event, EventBus, Result, SafetyLimits, TrainerCommand, timestamp_ms,
};
use futures_util::{FutureExt, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(crate) struct ActiveSession {
    pub io: FtmsSession,
    pub owner: Option<(usize, CancellationToken)>,
    permission: bool,
    synchronized: bool,
    ramp: Option<f32>,
    resistance: Option<f32>,
    last_resistance: Instant,
    assembler: RecordAssembler,
    power_decoder: crate::cycling_power::Decoder,
    last_parse_error: Option<Instant>,
    pub transport: Arc<dyn FtmsTransport>,
    pub bus: EventBus,
    pub id: String,
    pub limits: SafetyLimits,
}
pub(crate) enum SessionEvent {
    Telemetry(Option<Vec<u8>>),
    Control(Option<ControlEvent>),
}
impl ActiveSession {
    pub async fn next_event(&mut self) -> SessionEvent {
        tokio::select! {
            biased;
            event=async {match &mut self.io.control {Some(control)=>control.events.next().await,None=>std::future::pending().await}}=>SessionEvent::Control(event),
            packet=self.io.notifications.next()=>SessionEvent::Telemetry(packet),
        }
    }
    pub fn cancel_uncertain(&mut self) {
        self.synchronized = false;
    }
    pub fn new(
        io: FtmsSession,
        transport: Arc<dyn FtmsTransport>,
        bus: EventBus,
        id: String,
        limits: SafetyLimits,
    ) -> Self {
        Self {
            io,
            transport,
            bus,
            id,
            limits,
            owner: None,
            permission: false,
            synchronized: true,
            ramp: None,
            resistance: None,
            last_resistance: Instant::now(),
            assembler: RecordAssembler::default(),
            power_decoder: crate::cycling_power::Decoder::default(),
            last_parse_error: None,
        }
    }
    pub fn telemetry(&mut self, bytes: &[u8]) {
        let now = Instant::now();
        let decoded = match self.io.format {
            TelemetryFormat::Ftms => self.assembler.push(bytes, now, timestamp_ms()),
            TelemetryFormat::CyclingPower => self
                .power_decoder
                .decode(bytes, now, timestamp_ms())
                .map(Some),
            TelemetryFormat::HeartRate => {
                crate::heart_rate::decode(bytes, timestamp_ms()).map(Some)
            }
        };
        match decoded {
            Ok(Some(data)) => self.bus.publish(Event::Telemetry {
                device_id: self.id.clone(),
                data,
            }),
            Ok(None) => {}
            Err(error) => {
                if self
                    .last_parse_error
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5))
                {
                    self.bus.publish(Event::Error { data: error });
                    self.last_parse_error = Some(now);
                }
            }
        }
    }
    pub fn require_owner(&self, client: usize) -> Result<()> {
        if self
            .owner
            .as_ref()
            .is_some_and(|(owner, _)| *owner != client)
        {
            Err(denied("Another client owns trainer control."))
        } else {
            Ok(())
        }
    }
    pub fn status(&mut self, bytes: &[u8], pending: Option<u8>) -> Result<()> {
        let Some(opcode) = bytes.first() else {
            self.synchronized = false;
            return Err(invalid("Empty FTMS status notification."));
        };
        if *opcode == 2 {
            // Stop/Pause status can arrive after our Stop acknowledgement. It does not revoke control.
            // Always cancel a ramp so a user's stop cannot be followed by an automatic load increase.
            self.ramp = None;
            self.resistance = None;
            return Ok(());
        }
        if *opcode == 1 && !self.permission {
            return Ok(());
        }
        if *opcode == 0xff || (*opcode == 1 && pending != Some(1)) || *opcode == 3 {
            self.permission = false;
            self.ramp = None;
            if self.owner.is_none() {
                return Ok(());
            }
            return Err(denied(
                "Trainer control was revoked, reset, or stopped externally.",
            ));
        }
        Ok(())
    }
    /// Keep receiving telemetry during each write/indication transaction. Any ambiguous result poisons this session.
    async fn transact(&mut self, bytes: &[u8], cancel: &CancellationToken) -> Result<()> {
        // Reject queued responses: accepting one as the next acknowledgement would be unsafe.
        for _ in 0..64 {
            let ready = self
                .io
                .control
                .as_mut()
                .ok_or_else(|| unsupported("Trainer is telemetry-only."))?
                .events
                .next()
                .now_or_never();
            match ready {
                Some(Some(ControlEvent::Status(bytes))) => self.status(&bytes, None)?,
                Some(Some(ControlEvent::Indication(_))) => {
                    self.synchronized = false;
                    return Err(invalid("Unexpected queued FTMS control indication."));
                }
                Some(None) => {
                    self.synchronized = false;
                    return Err(disconnected());
                }
                None => break,
            }
        }
        self.synchronized = false;
        let transport = self.transport.clone();
        let write = transport.write_control(bytes);
        tokio::pin!(write);
        let deadline = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(deadline);
        let mut written = false;
        let mut indication: Option<Result<()>> = None;
        loop {
            if written && let Some(result) = indication.take() {
                self.synchronized =
                    !matches!(&result,Err(error) if error.code==ErrorCode::InvalidDeviceData);
                return result;
            }
            tokio::select! {
                biased;
                _=cancel.cancelled() => return Err(BridgeError::new(ErrorCode::TrainerControlFailed,"Control operation was cancelled; its outcome is uncertain.")),
                _=&mut deadline => return Err(BridgeError::new(ErrorCode::Timeout,"FTMS control acknowledgement timed out; reconnect before retrying.")),
                result=&mut write, if !written => {result?;written=true;},
                event=async { match self.io.control.as_mut() {Some(control)=>control.events.next().await,None=>None} } => {
                    match event {
                        Some(ControlEvent::Indication(response)) => {
                            if indication.is_some() {return Err(invalid("Duplicate FTMS control indication."));}
                            let result=decode_response(&response,bytes[0]);
                            if matches!(&result,Err(error) if error.code==ErrorCode::TrainerControlDenied) {self.permission=false;}
                            indication=Some(result);
                        }
                        Some(ControlEvent::Status(status)) => self.status(&status,Some(bytes[0]))?,
                        None => return Err(disconnected()),
                    }
                }
                packet=self.io.notifications.next() => match packet {Some(bytes)=>self.telemetry(&bytes),None=>return Err(disconnected())},
            }
        }
    }
    pub async fn execute(
        &mut self,
        client: usize,
        lease: CancellationToken,
        command: TrainerCommand,
        stop: &CancellationToken,
    ) -> Result<TrainerCommand> {
        self.require_owner(client)?;
        if lease.is_cancelled() {
            return Err(denied("Controlling client has left."));
        }
        let encoded = self
            .io
            .control
            .as_ref()
            .ok_or_else(|| unsupported("Trainer is telemetry-only."))?
            .profile
            .encode(self.limits, command)?;
        if !self.permission {
            // Claim local ownership before Request Control, so every partially completed request gets cleanup.
            self.owner = Some((client, lease.clone()));
            self.transact_cancelled(&[0], &lease, stop).await?;
            self.permission = true;
        }
        if matches!(command, TrainerCommand::RequestControl) {
            return Ok(command);
        }
        self.ramp = None;
        if let TrainerCommand::SetResistance(target) = encoded.applied {
            if self.limits.smooth_resistance {
                if self.resistance.is_none() {
                    let minimum = self
                        .io
                        .control
                        .as_ref()
                        .ok_or_else(|| unsupported("Trainer is telemetry-only."))?
                        .profile
                        .encode(self.limits, TrainerCommand::SetResistance(0.0))?;
                    self.transact_cancelled(&minimum.bytes, &lease, stop)
                        .await?;
                    self.resistance = Some(0.0);
                    self.last_resistance = Instant::now();
                }
                self.ramp = Some(target);
                return Ok(encoded.applied);
            }
        } else {
            self.resistance = None;
        }
        self.transact_cancelled(&encoded.bytes, &lease, stop)
            .await?;
        if matches!(command, TrainerCommand::Stop) {
            self.transact_cancelled(&[1], &lease, stop).await?;
            self.permission = false;
        }
        if let TrainerCommand::SetResistance(value) = encoded.applied {
            self.resistance = Some(value);
            self.last_resistance = Instant::now();
        }
        if matches!(command, TrainerCommand::Reset) {
            self.permission = false;
            self.owner = None;
            self.resistance = None;
        }
        Ok(encoded.applied)
    }
    async fn transact_cancelled(
        &mut self,
        bytes: &[u8],
        lease: &CancellationToken,
        stop: &CancellationToken,
    ) -> Result<()> {
        tokio::select! {
            biased;
            _=stop.cancelled() => {self.synchronized=false;Err(BridgeError::new(ErrorCode::TrainerControlFailed,"Daemon stopped during a control operation."))},
            result=self.transact(bytes,lease) => result,
        }
    }
    pub async fn advance_ramp(&mut self, stop: &CancellationToken) -> Result<()> {
        let (Some(target), Some(current), Some((_, lease))) =
            (self.ramp, self.resistance, self.owner.clone())
        else {
            return Ok(());
        };
        let maximum_step = self.last_resistance.elapsed().as_secs_f32()
            * self.limits.max_resistance_change_per_second;
        // Decreases bypass smoothing; increasing targets are bounded from the last confirmed write.
        let next = if target <= current {
            target
        } else {
            target.min(current + maximum_step)
        };
        let encoded = self
            .io
            .control
            .as_ref()
            .ok_or_else(|| unsupported("Trainer is telemetry-only."))?
            .profile
            .encode(self.limits, TrainerCommand::SetResistance(next))?;
        let TrainerCommand::SetResistance(applied) = encoded.applied else {
            return Ok(());
        };
        if applied == current {
            if current == target {
                self.ramp = None;
            }
            return Ok(());
        }
        self.transact_cancelled(&encoded.bytes, &lease, stop)
            .await?;
        self.resistance = Some(applied);
        self.last_resistance = Instant::now();
        if applied >= target {
            self.ramp = None;
        }
        Ok(())
    }
    /// Best effort Stop then Reset while the control channel is synchronized. Never reacquire revoked control.
    pub async fn safe_release(&mut self) {
        self.ramp = None;
        if self.permission && self.synchronized {
            let cancel = CancellationToken::new();
            if let Err(error) = self.transact(&[8, 1], &cancel).await {
                self.bus.publish(Event::Error { data: error });
            }
            if self.permission
                && self.synchronized
                && let Err(error) = self.transact(&[1], &cancel).await
            {
                self.bus.publish(Event::Error { data: error });
            }
        }
        self.owner = None;
        self.permission = false;
        self.resistance = None;
    }
}
pub(crate) fn denied(message: &str) -> BridgeError {
    BridgeError::new(ErrorCode::TrainerControlDenied, message)
}
pub(crate) fn disconnected() -> BridgeError {
    BridgeError::new(
        ErrorCode::DeviceDisconnected,
        "BLE telemetry connection was lost.",
    )
}
