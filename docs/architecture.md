# Architecture

BikeBridge runs as one native process. The first vertical slice is:

```text
MockTrainer / MockController
        ↓ normalized domain events
bikebridge-core EventBus (Tokio broadcast, capacity 256)
        ↓ per-client subscription and device filter
bikebridge-server (Axum HTTP/WebSocket)
        ↓ protocol v1 JSON
Rust console / Node.js / other native client
```

Commands travel in the reverse direction through strict parsing, device checks,
session ownership, and backend safety validation. The response returns the
effective command, so clamping is visible to the client.

## Crate boundaries

| Crate | Responsibility |
| --- | --- |
| `bikebridge-core` | Device and capability models, signed measurements, normalized inputs, trainer trait, safety limits, typed errors, domain events |
| `bikebridge-mock` | Deterministic MockTrainer, validated telemetry overrides, MockController |
| `bikebridge-ble` | Native discovery, service classification, opaque identity map, per-device FTMS sessions, control transactions, range mapping, and measurement decoding |
| `bikebridge-openbikecontrol` | Protocol decoding, input edges, held-state cleanup, independent controller workers |
| `bikebridge-trace` | Bounded recording writer, versioned trace validation, passive deterministic playback |
| `bikebridge-server` | Device state, control ownership, tick task, HTTP snapshots, JSON command schema, WebSocket subscriptions and lifecycle |
| `bikebridge-cli` | Config, argument overrides, logging, native listener, signals, status/device CLI queries |
| `bikebridge-example` | External WebSocket client using no server internals |

Core contains no GATT UUIDs, adapter handles, BLE addresses, Axum types, or platform
code. Its trainer interface returns Send futures and can be implemented by a BLE
backend without changing public commands. The control registry currently holds
concrete mock devices. BLE discovery maintains a separate registry, merged into
public device snapshots. Per-device BLE sessions overlay connection state and
verified capabilities on those snapshots. Discovery cannot send control commands.

The mock registry mutex is held only for short in-memory operations. Mock trainer
async methods never await I/O. The BLE backend uses per-device tasks and bounded
command queues/timeouts without holding this registry lock across BLE I/O.
Adapter and platform identifiers stay private to the transport registry.

## Lifecycle and concurrency

- The CLI validates configuration before binding a loopback listener.
- `--mock` constructs two connected development devices with fixed documented IDs.
- One 250 ms ticker advances the trainer and publishes per-device samples.
- Each WebSocket session has an independent bounded-bus receiver and subscription.
- No global mutable state or durable state is used.
- Device snapshots come from HTTP; the event bus has no replay or retained messages.
- Slow consumers receive an `events_lost` error. WebSocket writes time out after
  three seconds; message size is limited to 16 KiB. Sessions exceeding 100 text
  commands per one-second window are disconnected.
- WebSocket pings run every ten seconds. A client must respond with pongs; more
  than thirty seconds without a pong triggers cleanup at the next heartbeat.
- One client owns trainer control; all clients may subscribe. Ownership is first
  successful command wins and persists until reset, disconnect, or shutdown.
- Session teardown releases its owned trainer and decrements the active client count.
- Shutdown cancels the ticker and sessions, attempts load cleanup, and joins tasks.

## BLE discovery lifecycle (Phase 2)

`run` creates a `NativeBackend` backed by btleplug and attaches a `Scanner` actor.
`run --mock` does not construct a Bluetooth manager, enumerate radios, or request
OS Bluetooth permissions. Discovery can be tested with an injected `DiscoveryBackend`.

The actor serializes scan start/stop and adapter refresh with an eight-request
bounded queue. Each backend operation has a three-second deadline. Bluetooth waits
never hold server state locks; status/device snapshots are cached reads. Commands
whose callers leave before execution are skipped. A timed-out scan start triggers
best-effort stop because the platform operation may have partially succeeded.
If stopping fails, `scanning` stays true with `lastError` to represent uncertain
activity; a later explicit stop/start can retry. There is no automatic retry loop.

Default adapter selection prefers powered-on, then unknown, then powered-off radios.
A configured zero-based index overrides this selection. Enumeration occurs on
startup and on idle adapter-list/start requests. Adapter IDs are opaque session
UUIDs; adapter labels are generic because btleplug does not expose portable model
names. While scanning, retained handles are used without re-enumerating adapters.

Every second, the actor reads btleplug's advertisement-property cache. The backend
scans without service filters so manufacturer-only Click V2 advertisements can
be identified. The registry post-filters standard cycling services, Click V2 model
IDs, and explicitly selected names. UUIDs in service-data keys also count as advertised services.
Only cycling candidates are exposed. This snapshot-based discovery can coalesce
intermediate RSSI/name changes; it is not a raw advertisement stream. Scan loss
is detected when backend reads fail or report a powered-off radio. No platform
guarantee of continuous advertising reception is implied by an active scan.

The registry merges partial service advertisements and preserves name/RSSI when
an update omits them. New candidates emit `device.discovered`; changed metadata
emits `device.updated`. Identical snapshots emit nothing. Scan state changes emit
`scan.status`. Device records are capped at 1024 and retained after scanning stops.
There is no presence expiry, lost-device event, or durable identity persistence yet.

Private adapter/peripheral keys map to random public UUIDs. These stay stable across
scans in one daemon session. Platform handles remain inside `bikebridge-ble` for
explicit connections. Rotating peripheral identities or discovery via another
adapter may produce another public record; cross-adapter deduplication and identity
persistence are deferred. Device disappearance is not a connection/disconnection.

The discovery interface resolves a retained `FtmsTransport` without connecting.
Advertised roles remain provisional until actual GATT features are verified.
Shutdown cancels and joins discovery and attempts a bounded OS scan stop.

## FTMS session lifecycle (Phases 3–4)

An explicit connection lazily starts one worker for that device with a four-request
queue. Its injectable `FtmsTransport` provides open, connection checks, control writes, and disconnect;
the native implementation connects, discovers characteristics under the FTMS
service, validates READ/NOTIFY properties, reads features, installs a notification
receiver, and subscribes to Indoor Bike Data. When Control Point (WRITE/INDICATE)
and Machine Status (NOTIFY) are present, it installs a separate ordered control
stream and reads supported ranges. Missing control characteristics preserve a
telemetry-only session; malformed ranges disable the corresponding load mode. Open has a ten-second deadline. A partially failed or cancelled
open always attempts disconnect with a three-second deadline. Cleanup failures
remain observable and are retried before reopening the peripheral.

The worker serializes lifecycle changes and emits `device.connected` before any
telemetry. Notification bytes pass through an isolated decoder and record assembler,
then enter the existing bounded event bus as normalized measurements. Per-device
metadata locks are never held across BLE I/O. Advertisement updates preserve verified
capabilities and current connection state. Native adapter handles are retained
across enumeration so refreshing adapters does not replace active session handles.

Every second, a connected worker checks the OS link with a three-second deadline.
Failed checks and notification-stream closure end the session and clear partial
records. Silence alone is not treated as disconnect: a trainer may be asleep or
not pedaling. Optional automatic reconnection is bounded to five attempts with
1/2/5/10/30-second delays and emits `device.reconnecting` before each attempt.
Successful reconnection clears retry state and establishes telemetry without any
ownership or target replay. Explicit disconnect cancels retries.
Explicit disconnect and shutdown release sessions; viewer exit and scan stop leave
established connections available to others. A cancelled pending connect cleans up.

WebSocket connection and trainer requests run as a polled future alongside that socket's
event receiver and heartbeat. Only one can be pending per socket. HTTP and other
devices remain responsive during slow setup. The session task tracker joins all
device workers on shutdown. See [FTMS field and record rules](ftms.md).

## FTMS control transactions

Each device worker owns its Control Point transactions, exclusive client lease,
resistance ramp, and feature/range profile. Client teardown cancels the lease even
while a write is waiting. A procedure requires both a successful WithResponse write
and a matching indication, with a three-second total deadline. Telemetry is consumed
throughout. Wrong/duplicate opcodes, timeout, cancellation, or a failed write make
the outcome uncertain and close the connection without overlapping another write.

Control is requested before any load command. Unsupported commands are rejected
before acquisition. Other clients cannot change or disconnect an owned trainer.
Resistance increases use a 250 ms ticker and elapsed time since the last confirmed
step, snapped down to the device grid; entering resistance mode first establishes
the device minimum. Decreases bypass the slew limit. Mode changes cancel the ramp.
Each step waits for its own acknowledgement. Stop also resets FTMS permission;
local ownership remains until Reset or client exit.

Owner exit, explicit disconnect, execution failure, and shutdown attempt Stop then
Reset only when permission and transaction synchronization remain valid. Revoked
control is never automatically reacquired during cleanup. OS disconnect follows,
with its own deadline. Link failure can prevent all cleanup writes. See
[FTMS control](ftms-control.md) for wire choices and physical validation limits.

## Safety scope

Normalized resistance is a dimensionless 0–1 API value, **not** a raw FTMS level.
The BLE backend maps to verified supported resistance ranges and validates target
feature bits before encoding commands.

All trainer commands use `SafetyLimits`, also enforced by MockTrainer so callers
cannot bypass them by skipping the API. Safety configuration has hard sanity
ceilings: 2000 W, 25% absolute grade, and resistance 1.0. Default limits are lower.
Simulation wind is clamped to ±20 m/s, `crr` to 0–0.02, and `cw` to 0–1. Resistance
smoothing affects resistance mode. The mock ERG/simulation implementation generates
measurements, not physical forces; its reset clears targets and overrides immediately.

The mock's default rider still produces roughly 180 W after a reset, while its
applied resistance is zero. Resetting removes the load target; `trainer.stop` also
sets power, cadence, and speed to zero until `trainer.start`.

No physical trainer has been exercised. Injected transports verify transactions,
cancellation, limits, and cleanup; device-specific load response and fail-safe
behavior still require hardware acceptance testing.

## Security scope

Native local clients are trusted. The server rejects non-loopback listeners and
non-loopback Host headers, and rejects any Origin header to exclude browser clients
in this phase. No CORS, remote authentication, TLS, or LAN-control mode is provided.
These checks are not a sandbox against other local processes. Future remote access
must require explicit configuration and authentication.

## References

The implementation uses the current [Axum WebSocket API](https://docs.rs/axum/latest/axum/extract/ws/)
and [Tokio Tungstenite client API](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/).
Discovery uses [btleplug's Central API](https://docs.rs/btleplug/latest/btleplug/api/trait.Central.html)
and the four verified service UUIDs from the [Bluetooth SIG Assigned Numbers](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Assigned_Numbers/out/en/index-en.html).
FTMS field sources and the selected strict decoding policy are documented in
[FTMS telemetry](ftms.md). OpenBikeControl BLE sources and interoperability choices
are in [Zwift controller inputs](zwift-controllers.md).


## Recording and replay

`EventBus` has one optional nonblocking observer. Its short publication mutex
establishes identical event order for capture and broadcast. The recorder timestamps
and enqueues before fan-out, so subscriber lag cannot silently corrupt a capture.
A dedicated thread writes JSON Lines with periodic flushes. Failure is sticky;
the recording CLI monitors it, gracefully stops the daemon, and reports failure.
The recorder drains after all server sessions and BLE workers have shut down,
then writes and syncs a completion footer. Recording starts before discovery and
serving so the initial snapshot cannot race a producer.

Trainer API execution has an audit guard: start, applied/failed result, or cancelled
on future drop. This records the requested and effective commands across ownership
checks, clamping, hardware errors, and client departure without replaying actions.

Replay validates the complete bounded trace on a blocking thread before opening
its API listener. `AppState::replay` constructs neither mocks nor a native backend.
A five-millisecond scheduler advances the passive timeline and updates virtual
device snapshots. A paused timeline consumes no elapsed time. Restart publishes a
snapshot reset before replayed events. Recorded payload timestamps are preserved;
relative monotonic offsets drive scheduling, scaled by the requested speed.
The normal bounded subscriber bus still applies. No incoming or recorded command
can reach hardware through the replay backend. See [Recorder / Replay](recorder-replay.md).


## Controller bridge lifecycle

Discovery also filters OpenBikeControl advertisements and classifies them as
controllers. Native peripheral handles expose a separate `ControllerTransport`,
not the trainer interface. Per-device workers use bounded queues and deadlines,
subscribe before the app-info handshake, decode complete BLE notifications, and
emit only changed input states. Discovery cannot trigger an automatic connection.

A controller disconnect emits synthetic releases before its device event, ensuring
applications do not retain held buttons or brakes. Reconnection uses five bounded
backoff attempts and never restores held state. Viewer exit has no ownership effect;
trainer control commands are rejected for controller identities. The process-wide
recorder sees the same input/discovery/disconnect events as API subscribers.
