# Recorder / Replay

A `.biketrace` file captures the normalized BikeBridge API timeline. Share the file
with another developer and they can reproduce its events through the usual local
HTTP/WebSocket API without owning the original hardware. `.bike` is also accepted;
the file's versioned header determines its format, not its extension.

## Record a session

```sh
bikebridge run --record kickr-core-click-v2.biketrace
# Develop against ws://127.0.0.1:9376/ws as usual. Ctrl+C finalizes the recording.
```

This launches the daemon and starts recording before discovery or client sessions.
It records all devices, independently of individual clients' subscriptions. Use
`--mock` for a hardware-free recording or `--duration 60` to stop and finalize after
60 seconds of serving. The destination must be a new file; existing files are never
overwritten. The filename is descriptive only and does not claim device support.

Captured information includes:

- Initial device identities, names, capabilities, and connection state.
- Every published telemetry field, including power, cadence, speed, heart rate,
  distance, and resistance where the backend actually supplies it.
- Normalized controller inputs such as shifting and steering.
- Trainer command requests and their applied, failed, or cancelled outcomes.
- Device discovery, metadata changes, disconnects, reconnect attempts, scan state,
  public errors, and API client disconnects.

The recorder captures public API events, not raw BLE packets, private transport
identifiers, log output, malformed API requests, or every internal resistance-ramp
write. Shifting requires an input backend; today it can be injected through
MockController or the [BikeControl/OpenBikeControl BLE bridge](zwift-controllers.md).
Physical controller compatibility through the bridge still needs hardware validation.
[Di2 and AXS demo traces](di2-axs.md#try-without-hardware) provide synthetic mapped
shifter inputs, held-button cleanup, and bridge reconnection without hardware.
Recording does not invent measurements absent from the device.

## Replay without hardware

```sh
bikebridge trace-info kickr-core-click-v2.biketrace
bikebridge replay kickr-core-click-v2.biketrace --speed 1
```

Replay starts paused. Connect your app, obtain `/api/devices`, and subscribe to its
events. Then, in another terminal:

```sh
bikebridge replay-control start
bikebridge replay-control pause
bikebridge replay-control start
bikebridge replay-control restart
```

Use `--port 9380` consistently when a live daemon already occupies the default
port. `--speed` accepts 0.1–16; `--autoplay` starts immediately when that is useful.
At EOF the daemon stays available with `replay.finished: true` until restarted or
stopped. Start at EOF is a no-op; restart rewinds and starts again. Restart emits
`replay.reset` containing the initial device snapshot, so clients can clear stale
state. HTTP snapshots subsequently follow recorded device lifecycle events.

Original payloads, measurement timestamps, device IDs, event order, and relative
timing are preserved. Playback speed changes scheduling, not payload timestamps.
The scheduler checks every five milliseconds; OS scheduling and WebSocket buffering
can add jitter. Pausing freezes timeline progress. Equal-time events preserve their
recorded sequence order. Existing subscribers still have normal bounded-buffer
semantics and must handle `events_lost` at high replay rates.

Replay constructs no Bluetooth backend. Recorded commands are published as events,
never sent to hardware. New trainer control commands return `unsupported_operation`.
Connect/disconnect requests succeed only when they already match the recorded
state; the timeline owns connection changes. This version is passive API-event
reproduction, not interactive command matching, trainer physics, or a BLE stack
emulator. It can reproduce application behavior driven by the recorded timeline;
it cannot reproduce a radio/firmware bug by itself.

## Try the included demo

```sh
bikebridge replay examples/traces/demo-ride.biketrace
node examples/javascript/replay.mjs
```

The Node.js 22+ viewer subscribes to all event families, starts playback, prints
events, and exits after EOF. The hand-authored, 4.5-second mock fixture includes
power/cadence/HR, a shift, resistance and ERG commands, a clamped target, disconnect,
reconnect, and client departure. It is not a recording from a physical trainer.

## HTTP and WebSocket additions

`GET /api/replay` returns playback status, or `null` outside replay mode.
`POST /api/replay/start`, `/pause`, and `/restart` take an empty body and return the
same status. `GET /api/status` includes a `replay` field only in replay mode:

```json
{"playing":false,"finished":false,"speed":1.0,"positionUs":0,"durationUs":4500000,"emittedEvents":0,"totalEvents":12}
```

Three additive subscription families are `command`, `session`, and `replay`.
Device filters apply to commands; session disconnects and replay resets have no
device ID and require an empty/omitted device filter. Existing subscriptions are
unchanged. Trainer commands publish these events even when recording is disabled:

```json
{"type":"command.started","commandId":7,"sessionId":2,"deviceId":"mock-trainer","command":{"operation":"set_target_power","value":900}}
{"type":"command.finished","commandId":7,"deviceId":"mock-trainer","outcome":{"status":"applied","command":{"operation":"set_target_power","value":800}}}
{"type":"session.disconnected","sessionId":2}
```

Failures have `outcome: {"status":"failed","error":{"code":"…","message":"…"}}`;
cancelled futures have `outcome: {"status":"cancelled"}`. Cancellation does not
imply that a hardware command did not take effect. IDs correlate events within a
single daemon run. A successful smoothed resistance command still means an accepted
target, not completion of its ramp.

## File format and integrity

Version 1 is UTF-8 JSON Lines with a final newline on each record:

1. A `record: "header"` row identifies `format: "bikebridge.trace"`, schema version,
   API protocol version, daemon version, original wall-clock start, and initial devices.
2. `record: "event"` rows contain contiguous zero-based `sequence`, monotonic
   `offsetUs`, and the original `event` object.
3. A `record: "end"` footer contains the event count and total `durationUs`.

The recorder timestamps at publication and preserves the same order delivered to
the event bus. An 8192-event queue separates hardware handling from disk writes.
It flushes at least once per second and drains, flushes, and syncs on normal stop.
Queue overflow, disk errors, or resource-limit failure stop the recording daemon
with an error; an incomplete trace is never silently presented as successful.
Forced process termination or a crash can leave a partial file. `trace-info` and
replay reject files missing the completion footer; partial-file recovery is not
implemented.

Loading validates the format, sequence, monotonic offsets, completion count, and
size bounds: 64 MiB, one million events, one MiB per line, seven days. This is
structural integrity checking, not a cryptographic signature. Files are deliberately
readable and editable for deterministic test fixtures.
