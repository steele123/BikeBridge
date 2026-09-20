use crate::Trace;
use anyhow::{Result, ensure};
use bikebridge_core::{DeviceInfo, Event};
use serde::Serialize;
use std::{collections::BTreeMap, time::Duration};

/// Current timeline state exposed in /api/status and /api/replay.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStatus {
    /// Whether the timeline advances.
    pub playing: bool,
    /// Whether the end has been reached and all events have been emitted.
    pub finished: bool,
    /// Playback multiplier, 0.1 through 16.
    pub speed: f64,
    /// Current original-timeline position in microseconds.
    pub position_us: u64,
    /// Original total duration.
    pub duration_us: u64,
    /// Events emitted since the last restart.
    pub emitted_events: usize,
    /// Total events in the file.
    pub total_events: usize,
}

/// Hardware-free passive timeline. Commands in records are emitted, never executed.
pub struct Playback {
    trace: Trace,
    devices: BTreeMap<String, DeviceInfo>,
    status: PlaybackStatus,
    fractional_us: f64,
}
impl Playback {
    /// Begin paused so applications can connect and subscribe before time zero.
    pub fn new(trace: Trace, speed: f64) -> Result<Self> {
        ensure!(
            speed.is_finite() && (0.1..=16.0).contains(&speed),
            "Replay speed must be between 0.1 and 16"
        );
        let status = PlaybackStatus {
            playing: false,
            finished: false,
            speed,
            position_us: 0,
            duration_us: trace.duration_us(),
            emitted_events: 0,
            total_events: trace.records().len(),
        };
        let devices = trace
            .header()
            .devices
            .iter()
            .map(|d| (d.id.clone(), d.clone()))
            .collect();
        Ok(Self {
            trace,
            devices,
            status,
            fractional_us: 0.0,
        })
    }
    /// Start/resume; after EOF use restart to play again.
    pub fn start(&mut self) {
        if !self.status.finished {
            self.status.playing = true;
        }
    }
    /// Pause without consuming timeline time.
    pub fn pause(&mut self) {
        self.status.playing = false;
    }
    /// Restore the original snapshot and start at time zero.
    pub fn restart(&mut self) -> Event {
        self.status.position_us = 0;
        self.status.emitted_events = 0;
        self.status.finished = false;
        self.status.playing = true;
        self.fractional_us = 0.0;
        self.devices = self
            .trace
            .header()
            .devices
            .iter()
            .map(|d| (d.id.clone(), d.clone()))
            .collect();
        Event::ReplayReset {
            devices: self.devices(),
        }
    }
    /// Current virtual device snapshot; original identities and metadata are preserved.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.values().cloned().collect()
    }
    /// Current timeline state.
    pub fn status(&self) -> PlaybackStatus {
        self.status.clone()
    }
    /// Advance by wall elapsed time, returning due events in original order.
    /// At most 2048 events are emitted per call; later calls drain any backlog.
    pub fn advance(&mut self, elapsed: Duration) -> Vec<Event> {
        if !self.status.playing {
            return Vec::new();
        }
        let micros = elapsed.as_secs_f64() * 1_000_000.0 * self.status.speed + self.fractional_us;
        self.fractional_us = micros.fract();
        self.status.position_us = self
            .status
            .position_us
            .saturating_add(micros as u64)
            .min(self.status.duration_us);
        let mut due = Vec::new();
        while let Some(record) = self.trace.records().get(self.status.emitted_events) {
            if record.offset_us > self.status.position_us || due.len() == 2048 {
                break;
            }
            match &record.event {
                Event::DeviceDiscovered { data }
                | Event::DeviceUpdated { data }
                | Event::DeviceConnected { data }
                | Event::DeviceDisconnected { data } => {
                    self.devices.insert(data.id.clone(), data.clone());
                }
                _ => {}
            }
            due.push(record.event.clone());
            self.status.emitted_events += 1;
        }
        if self.status.position_us == self.status.duration_us
            && self.status.emitted_events == self.status.total_events
        {
            self.status.playing = false;
            self.status.finished = true;
        }
        due
    }
}
