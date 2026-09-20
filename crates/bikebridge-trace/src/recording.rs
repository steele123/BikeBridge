use crate::{
    Header, Line, MAX_BYTES, MAX_DURATION_US, MAX_EVENTS, MAX_LINE, Record, validate_header,
};
use anyhow::{Context, Result, bail, ensure};
use bikebridge_core::{DeviceInfo, Event, EventBus, EventObserver, timestamp_ms};
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Counts returned only after the final footer is flushed and synced.
#[derive(Debug)]
pub struct RecordingSummary {
    /// Number of recorded events.
    pub events: u64,
    /// Total duration in microseconds.
    pub duration_us: u64,
}

#[derive(Debug)]
struct Capture {
    sender: SyncSender<(u64, Event)>,
    started: Instant,
    failed: Arc<AtomicBool>,
    end_us: Arc<AtomicU64>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.end_us.store(
            self.started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
    }
}
impl EventObserver for Capture {
    fn observe(&self, event: &Event) {
        if self.failed.load(Ordering::Acquire) {
            return;
        }
        let offset = self.started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        if self.sender.try_send((offset, event.clone())).is_err() {
            // Never silently drop an event or block a trainer transaction on disk I/O.
            self.failed.store(true, Ordering::Release);
        }
    }
}

/// Ordered recorder with an 8192-event bounded queue and a dedicated disk writer.
/// Start before discovery/server tasks to make the initial snapshot race-free.
pub struct Recorder {
    bus: EventBus,
    failed: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<RecordingSummary>>>,
}
impl Recorder {
    /// Create a new file exclusively; existing traces are never overwritten.
    pub fn start(path: impl AsRef<Path>, bus: EventBus, devices: Vec<DeviceInfo>) -> Result<Self> {
        let header = Header {
            format: "bikebridge.trace".into(),
            version: 1,
            protocol_version: 1,
            bike_bridge_version: env!("CARGO_PKG_VERSION").into(),
            created_at_ms: timestamp_ms(),
            devices,
        };
        validate_header(&header)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .context("Cannot create trace (choose a new output path)")?;
        let mut writer = Writer {
            file: BufWriter::new(file),
            bytes: 0,
        };
        writer.line(&Line::Header { data: header })?;
        writer.file.flush()?;
        let (sender, receiver) = mpsc::sync_channel(8192);
        let failed = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let end_us = Arc::new(AtomicU64::new(0));
        let capture = Arc::new(Capture {
            sender,
            started,
            failed: failed.clone(),
            end_us: end_us.clone(),
        });
        bus.observe(capture)?;
        let failure = failed.clone();
        let worker = std::thread::Builder::new().name("bikebridge-recorder".into()).spawn(move || {
            let result = (|| {
                let mut events = 0;
                let mut last_flush = Instant::now();
                loop {
                    if failure.load(Ordering::Acquire) { bail!("Recording failed: event queue overflow or writer failure; trace is incomplete"); }
                    match receiver.recv_timeout(Duration::from_millis(250)) {
                        Ok((offset_us, event)) => {
                            ensure!((events as usize) < MAX_EVENTS && offset_us <= MAX_DURATION_US, "Recording exceeded event/duration limits");
                            writer.line(&Line::Event { data: Record { sequence: events, offset_us, event } })?;
                            events += 1;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if last_flush.elapsed() >= Duration::from_secs(1) {
                        writer.file.flush()?;
                        last_flush = Instant::now();
                    }
                }
                ensure!(!failure.load(Ordering::Acquire), "Recording lost events; trace is incomplete");
                let duration_us = end_us.load(Ordering::Acquire);
                ensure!(duration_us <= MAX_DURATION_US, "Recording exceeded seven days");
                writer.line(&Line::End { events, duration_us })?;
                writer.file.flush()?;
                writer.file.get_ref().sync_all()?;
                Ok(RecordingSummary { events, duration_us })
            })();
            if result.is_err() { failure.store(true, Ordering::Release); }
            result
        });
        match worker {
            Ok(worker) => Ok(Self {
                bus,
                failed,
                worker: Some(worker),
            }),
            Err(error) => {
                bus.stop_observing();
                Err(error.into())
            }
        }
    }
    /// Whether capture overflowed or disk writing failed. The daemon should stop recording visibly.
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// Detach, drain queued events, write the completion footer, and sync the file.
    /// This waits for disk I/O; call on a blocking thread from async code.
    pub fn finish(mut self) -> Result<RecordingSummary> {
        self.bus.stop_observing();
        self.worker
            .take()
            .context("Recorder already finished")?
            .join()
            .map_err(|_| anyhow::anyhow!("Recorder thread panicked"))?
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.bus.stop_observing();
            let _ = worker.join();
        }
    }
}
struct Writer {
    file: BufWriter<File>,
    bytes: u64,
}
impl Writer {
    fn line(&mut self, line: &Line) -> Result<()> {
        let mut bytes = serde_json::to_vec(line)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() <= MAX_LINE,
            "Trace record exceeded line size limit"
        );
        self.bytes += bytes.len() as u64;
        ensure!(self.bytes <= MAX_BYTES, "Recording exceeded 64 MiB");
        self.file.write_all(&bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_marks_capture_failed_instead_of_silently_dropping() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let failed = Arc::new(AtomicBool::new(false));
        let capture = Capture {
            sender,
            started: Instant::now(),
            failed: failed.clone(),
            end_us: Arc::new(AtomicU64::new(0)),
        };
        capture.observe(&Event::SessionDisconnected { session_id: 1 });
        assert!(!failed.load(Ordering::Acquire));
        capture.observe(&Event::SessionDisconnected { session_id: 2 });
        assert!(failed.load(Ordering::Acquire));
    }
}
