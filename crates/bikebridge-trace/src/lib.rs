//! Versioned, bounded JSON-lines session recording and deterministic passive replay.
mod playback;
mod recording;
pub use playback::{Playback, PlaybackStatus};
pub use recording::{Recorder, RecordingSummary};

use anyhow::{Context, Result, bail, ensure};
use bikebridge_core::{DeviceInfo, Event};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINE: usize = 1024 * 1024;
const MAX_EVENTS: usize = 1_000_000;
const MAX_DURATION_US: u64 = 7 * 24 * 60 * 60 * 1_000_000;

/// Initial snapshot and format version. No platform BLE addresses are included.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    /// Format identity, always bikebridge.trace.
    pub format: String,
    /// Trace schema version, currently 1.
    pub version: u8,
    /// BikeBridge protocol version, currently 1.
    pub protocol_version: u8,
    /// Recording daemon version.
    pub bike_bridge_version: String,
    /// Original wall-clock start time.
    pub created_at_ms: u64,
    /// Devices already present when recording began.
    pub devices: Vec<DeviceInfo>,
}

/// One normalized API event with its original publication time and order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    /// Contiguous zero-based sequence number, resolving equal timestamps.
    pub sequence: u64,
    /// Monotonic microseconds since recording started.
    pub offset_us: u64,
    /// Original event, including its original measurement timestamp.
    pub event: Event,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
enum Line {
    Header {
        #[serde(flatten)]
        data: Header,
    },
    Event {
        #[serde(flatten)]
        data: Record,
    },
    End {
        events: u64,
        #[serde(rename = "durationUs")]
        duration_us: u64,
    },
}

/// Fully validated trace. Loading never accesses Bluetooth or executes commands.
#[derive(Debug)]
pub struct Trace {
    header: Header,
    records: Vec<Record>,
    duration_us: u64,
}
impl Trace {
    /// Load a complete trace, rejecting truncation, unsupported versions, and resource excess.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path).context("Cannot open trace")?;
        ensure!(
            file.metadata()?.len() <= MAX_BYTES,
            "Trace exceeds the 64 MiB limit"
        );
        Self::read(file)
    }
    /// Read and validate a bounded JSON-lines stream.
    pub fn read(reader: impl Read) -> Result<Self> {
        let mut reader = BufReader::new(reader);
        let mut total = 0u64;
        let mut number = 0;
        let mut header = None;
        let mut records: Vec<Record> = Vec::new();
        let mut end = None;
        loop {
            let mut bytes = Vec::new();
            let size = reader
                .by_ref()
                .take((MAX_LINE + 1) as u64)
                .read_until(b'\n', &mut bytes)?;
            if size == 0 {
                break;
            }
            number += 1;
            total += size as u64;
            ensure!(
                size <= MAX_LINE && total <= MAX_BYTES,
                "Trace size limit exceeded at line {number}"
            );
            ensure!(
                bytes.last() == Some(&b'\n'),
                "Truncated trace line {number}"
            );
            ensure!(end.is_none(), "Data follows trace footer");
            let line: Line = serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid trace line {number}"))?;
            match line {
                Line::Header { data } => {
                    ensure!(
                        number == 1,
                        "Trace header must appear exactly once at the beginning"
                    );
                    validate_header(&data)?;
                    header = Some(data);
                }
                Line::Event { data } => {
                    ensure!(header.is_some(), "Trace must start with a header");
                    ensure!(records.len() < MAX_EVENTS, "Trace event limit exceeded");
                    ensure!(
                        data.sequence == records.len() as u64,
                        "Missing or reordered trace event at line {number}"
                    );
                    ensure!(
                        data.offset_us <= MAX_DURATION_US,
                        "Trace duration exceeds seven days"
                    );
                    ensure!(
                        records
                            .last()
                            .is_none_or(|last| last.offset_us <= data.offset_us),
                        "Nonmonotonic event time at line {number}"
                    );
                    ensure!(
                        !matches!(data.event, Event::ReplayReset { .. }),
                        "Replay lifecycle events cannot appear in a live trace"
                    );
                    records.push(data);
                }
                Line::End {
                    events,
                    duration_us,
                } => {
                    ensure!(header.is_some(), "Trace footer has no header");
                    ensure!(
                        events == records.len() as u64,
                        "Trace footer count mismatch"
                    );
                    ensure!(
                        duration_us <= MAX_DURATION_US
                            && records.last().is_none_or(|r| r.offset_us <= duration_us),
                        "Invalid trace duration"
                    );
                    end = Some(duration_us);
                }
            }
        }
        let Some(duration_us) = end else {
            bail!("Incomplete trace: missing completion footer (recording interrupted or failed)");
        };
        Ok(Self {
            header: header.context("Missing trace header")?,
            records,
            duration_us,
        })
    }
    /// Initial recording metadata.
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Validated events in original order.
    pub fn records(&self) -> &[Record] {
        &self.records
    }
    /// Total monotonic recording duration, including its quiet tail.
    pub fn duration_us(&self) -> u64 {
        self.duration_us
    }
}

fn validate_header(header: &Header) -> Result<()> {
    ensure!(
        header.format == "bikebridge.trace" && header.version == 1 && header.protocol_version == 1,
        "Unsupported BikeBridge trace format or version"
    );
    ensure!(header.devices.len() <= 1024, "Too many initial devices");
    let mut ids = HashSet::new();
    for device in &header.devices {
        ensure!(
            !device.id.is_empty() && device.id.len() <= 128 && ids.insert(&device.id),
            "Invalid or duplicate initial device ID"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
