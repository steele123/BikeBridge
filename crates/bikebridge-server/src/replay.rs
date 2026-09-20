use bikebridge_core::{BridgeError, DeviceInfo, ErrorCode, EventBus, Result};
use bikebridge_trace::{Playback, PlaybackStatus, Trace};
use tokio::time::Instant;

pub(crate) struct ReplayRuntime {
    playback: Playback,
    last: Instant,
}
impl ReplayRuntime {
    pub fn new(trace: Trace, speed: f64) -> anyhow::Result<Self> {
        Ok(Self {
            playback: Playback::new(trace, speed)?,
            last: Instant::now(),
        })
    }
    pub fn status(&self) -> PlaybackStatus {
        self.playback.status()
    }
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.playback.devices()
    }
    pub fn tick(&mut self, events: &EventBus) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last);
        self.last = now;
        for event in self.playback.advance(elapsed) {
            events.publish(event);
        }
    }
    pub fn action(&mut self, action: &str, events: &EventBus) -> Result<PlaybackStatus> {
        if !matches!(action, "start" | "pause" | "restart") {
            return Err(BridgeError::new(
                ErrorCode::InvalidCommand,
                "Replay action must be start, pause, or restart.",
            ));
        }
        self.tick(events);
        match action {
            "start" => self.playback.start(),
            "pause" => self.playback.pause(),
            "restart" => events.publish(self.playback.restart()),
            _ => unreachable!(),
        }
        Ok(self.status())
    }
}
