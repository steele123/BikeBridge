# BikeBridge protocol v1

Connect a client to `ws://127.0.0.1:9376/ws`. UTF-8 JSON text frames only.
The daemon sends `hello` before any other application message:

```json
{"type":"hello","protocolVersion":1,"bikeBridgeVersion":"0.1.0"}
```

Clients should check `protocolVersion`, ignore unfamiliar additive event fields,
and implement WebSocket ping/pong (most WebSocket libraries do this automatically).
The dashboard served at `/` uses this same API. Browser requests must have the
exact same HTTP origin as the daemon (including host and port); foreign and null
origins are rejected. Native clients without an Origin header and Node's built-in
WebSocket are supported.

### Select a discovery name

`POST /api/scan/select` accepts `{"name":"Steele's Bike"}` and returns the scan
status. The name must contain 1–128 UTF-8 bytes after trimming, with no control
characters. This adds a name for the current daemon session and restarts scanning
without service filters. Only recognized cycling devices, identified Click V2
controllers, and explicitly selected names appear in discovery results. Matching ignores case, surrounding whitespace,
and straight/curly apostrophe differences. Selection does not connect the device
or grant capabilities. Persist names with `[bluetooth].device_names` in the daemon
configuration. Mock/replay modes do not support Bluetooth name selection.

## Requests and responses

### Nearby Bluetooth browser

`GET /api/scan/nearby` returns a cached array with `id`, `name` (nullable),
`signalStrength` (nullable), and `deviceId` (nullable). The `nearby-…` selection ID
is opaque and session-scoped; `deviceId` links to `/api/devices` when a result is
already recognized or explicitly selected. This includes unrelated/unsupported
Bluetooth devices. It does not expose addresses or raw manufacturer payloads.

`POST /api/scan/nearby/{id}/select` with an empty body selects exactly that result
and starts scanning if necessary, returning scan status. It does not connect or
grant capabilities. Unlike name selection, it distinguishes duplicate and unnamed
devices. Unknown IDs return 404 without interrupting scanning. Selection lasts
until daemon shutdown.

Results are bounded to 1024 cached identities, updated while scanning, and retained
when scanning stops. OS advertisement caches can retain devices that are no longer
nearby; no last-seen timestamp is claimed. Names/RSSI survive partial updates.
Mock/replay sessions return an empty list and do not support selection. The same
loopback/same-origin restrictions apply as for the rest of the API.

### Command envelopes

Every command has `type`. A `requestId` is optional; when present it must be a
nonempty string of at most 128 UTF-8 bytes. Every command gets a `response`, even
when it has no request ID. Clients should use distinct IDs to correlate concurrent
commands. IDs do not provide deduplication or idempotency.

```json
{"type":"response","requestId":"a","success":true,"data":{}}
```

```json
{"type":"response","requestId":"a","success":false,"error":{"code":"device_not_found","message":"Device not found."}}
```

Unknown commands/fields, wrong types, missing required fields, invalid JSON, and
binary frames are rejected. Malformed messages do not execute commands. An invalid
or unparseable request ID cannot be echoed. Maximum message and frame size: 16 KiB.
More than 100 text commands in a one-second window closes the connection.

## Subscriptions

Sessions initially subscribe to nothing. Each `subscribe` **replaces** both filters:

```json
{"type":"subscribe","requestId":"sub","events":["telemetry","input","device"],"deviceIds":["mock-trainer","mock-controller"]}
```

Event families are `telemetry`, `input`, `device`, `scan`, `error`, `command`,
`session`, and `replay`. `device` covers all
`device.*` messages. Omitted or empty `deviceIds` means all devices; an empty
`events` array unsubscribes from everything. At most eight event categories and 64
device filters (each 1–128 bytes) are accepted. Device filters do not prevent
direct command responses. A device filter excludes events with no device identity.

There is no initial snapshot or replay. Use HTTP to list current devices and then
consume subsequent events. Subscribe before fetching a snapshot if you need to
reconcile events that race with the fetch. Mock devices already exist at startup,
so clients should not wait for an initial discovery event. After bus lag, the
server sends `error` with `events_lost` regardless of the subscription; refresh
device state via HTTP. Telemetry samples are replaceable; input edges are not.

## Device commands

```json
{"type":"device.disconnect","requestId":"d1","deviceId":"mock-trainer"}
```

```json
{"type":"device.connect","requestId":"c1","deviceId":"mock-trainer"}
```

Both return the device snapshot in `response.data`. Repeating the current state
is a no-op; a changed state emits one event to subscribers:

```json
{"type":"device.connected","data":{"id":"mock-trainer","name":"BikeBridge Mock Trainer","kind":"trainer","transport":"mock","connected":true,"capabilities":["power","cadence","speed","heart_rate","resistance_control","erg_control","simulation_control"]}}
```

`device.disconnected` uses the same shape with `connected: false`.
The controlling client alone may disconnect/reconnect its owned trainer. Controller
connections are not leased. IDs are opaque; the fixed mock IDs are development
fixtures and do not imply future BLE identity formats.

Discovered BLE devices have `transport: "bluetooth"`, opaque `ble-…` IDs,
`connected: false`, and initially empty `capabilities`. Their advertised role is
provisional. FTMS connection verifies telemetry and control capabilities; unsupported
control modes return `unsupported_operation`. `device.updated` has the same `data`
shape as `device.discovered`, and is emitted when name, RSSI, or classification
changes. Missing advertisement fields do not erase previously seen metadata.

For an FTMS candidate, `device.connect` discovers services, validates and reads
Fitness Machine Feature, and subscribes to Indoor Bike Data before returning a
successful response with the connected DeviceInfo. Capabilities include `speed`
and feature-confirmed `power`, `cadence`, or `heart_rate`. With a usable Control Point
and Machine Status channel, supported target bits and valid ranges add
`resistance_control`, `erg_control`, or `simulation_control`. HTTP `POST /api/devices/{id}/connect` and `/disconnect` perform
the same operation with an empty body and return DeviceInfo directly.

Manually named candidates may initially have `kind: "unknown"` and no capabilities.
Connection verifies their GATT services. If FTMS Indoor Bike Data is unavailable,
the driver accepts readable Cycling Power Feature and notifying Cycling Power
Measurement under service `0x1818`. A successful Cycling Power connection reports
`kind: "power_meter"`, `power`, and optional `cadence`, with no trainer control
capabilities. CLI name selection resolves to the existing opaque ID, so the wire
commands are unchanged.

```json
{"type":"device.connect","requestId":"connect-1","deviceId":"ble-<opaque-uuid>"}
```

BLE sessions are shared by all local clients and persist when a viewer closes.
Explicit disconnect, detected link loss, or daemon shutdown ends a session;
stopping discovery does not. Repeated connect/disconnect requests are idempotent.
Owner exit also closes its controlled BLE connection. Reconnection is explicit unless
`[trainer].auto_reconnect = true`: link loss schedules at most five retries with
1, 2, 5, 10, and 30 second delays. Each emits a device-filterable event:

```json
{"type":"device.reconnecting","deviceId":"ble-<opaque-uuid>","attempt":1,"delaySeconds":1}
```

Successful reconnection restores telemetry only. A client must reacquire control
and explicitly send any new target. Explicit disconnect cancels retries; owner exit
and control-command failures do not start retries. Previously verified capabilities remain cached when
disconnected and are refreshed at the next successful connection. `connected`
means a usable telemetry session; failed OS teardown clears it and also emits
an error because underlying OS connection state may be uncertain.

Connect setup has a ten-second deadline; teardown and connection-state checks
have three-second deadlines. Each control write plus matching indication has a
three-second total deadline. A WebSocket accepts one pending connection or trainer
command while continuing to deliver events and process pings and subscriptions;
another device operation on that socket gets `busy`. Leaving during setup cancels the
request and attempts cleanup. Events and command responses may interleave.
Connection errors are returned to the caller and published as `error` events;
they do not overwrite `scan.lastError`.

HTTP connection errors use 404 for unknown IDs, 422 for unsupported devices,
409 for control ownership conflicts, 429 for a full queue, 504 for timeouts,
and 503 for other device failures. HTTP connections cannot bypass an existing
WebSocket client's trainer ownership.

## Bluetooth discovery

```json
{"type":"subscribe","requestId":"discovery","events":["device","scan","error"]}
```

Start or stop scanning using `POST /api/scan/start` and `POST /api/scan/stop`.
Both take an empty body, are idempotent, and return scan status directly:

```json
{"scanning":true,"adapterId":"adapter-<opaque-uuid>","lastError":null}
```

Subscribers also receive:

```json
{"type":"scan.status","data":{"scanning":true,"adapterId":"adapter-<opaque-uuid>","lastError":null}}
```

Scan events have no device ID, so a nonempty subscription `deviceIds` filter
excludes them. Scanning is process-wide, not owned by a WebSocket session. Client
disconnect does not stop scanning. Daemon shutdown does. Scanning continues until
stopped, a backend failure occurs, or the daemon exits. After a backend failure,
explicitly start again to retry; there is no aggressive background restart loop.

`scanning: true` with a non-null `lastError` means the stop attempt failed and OS
scan activity is uncertain. A successful restart or stopping an active scan clears
the error. Refreshing adapter metadata preserves the last scan error for diagnosis.
Adapter metadata is refreshed on idle `GET /api/adapters` and scan-start requests;
while scanning it reflects the last enumeration and subsequent detected failures.

`GET /api/adapters` returns records such as:

```json
[{"id":"adapter-<opaque-uuid>","name":"Bluetooth adapter 1","state":"powered_on","isDefault":true}]
```

States are `powered_on`, `powered_off`, or `unknown`. The first powered-on radio
is preferred unless `bluetooth.adapter_index` / `run --adapter-index` selects an
explicit zero-based index. Adapter and device IDs last only for this daemon
session. Stop/start retains discovered records and IDs; restarting the daemon
does not. Records represent discoveries, not proof that devices remain in range.

Failures use an HTTP error envelope and non-2xx status:

```json
{"type":"error","data":{"code":"adapter_not_found","message":"No matching Bluetooth adapter found."}}
```

Unavailable Bluetooth, missing adapters, and platform scan failures return 503;
operation deadlines return 504; a full command queue returns 429. With an empty
device list, `scan.lastError: null` distinguishes a healthy empty scan from failure.
Mock mode returns no adapters and rejects scan requests with `bluetooth_unavailable`.

## Trainer commands

| Type | Required `data` | Effective result |
| --- | --- | --- |
| `trainer.requestControl` | No data field | Acquire exclusive ownership |
| `trainer.reset` | No data field | Clear load and overrides; release ownership |
| `trainer.start` | No data field | Start/resume workout (simulated pedaling in mock mode) |
| `trainer.stop` | No data field | Stop and reset BLE trainer; keep local ownership (stop simulated pedaling in mock mode) |
| `trainer.setResistance` | `{"resistance":0.35}` | Clamp normalized resistance to 0…configured maximum |
| `trainer.setTargetPower` | `{"watts":250}` | Clamp unsigned 16-bit target to configured maximum |
| `trainer.setSimulation` | See below | Clamp simulation parameters |

These commands apply to MockTrainer and connected FTMS trainers with control support.
All require `deviceId`.
The first successful command implicitly acquires control;
`requestControl` is useful for acquiring it explicitly. Another client's commands
fail with `trainer_control_denied`. Reset releases ownership. Owner disconnect
clears mock targets; for BLE it attempts Stop/Reset and closes the connection.
A telemetry-only client disconnect does not affect the owner. BLE Stop also resets
FTMS permission while retaining local ownership; the next command reacquires it.
See [FTMS control](ftms-control.md) for exact failure and cleanup behavior.

```json
{"type":"trainer.setResistance","deviceId":"mock-trainer","requestId":"r1","data":{"resistance":0.35}}
```

```json
{"type":"trainer.setTargetPower","deviceId":"mock-trainer","requestId":"erg1","data":{"watts":250}}
```

```json
{"type":"trainer.setSimulation","deviceId":"mock-trainer","requestId":"sim1","data":{"gradePercent":6.5,"windSpeedMps":0.0,"crr":0.004,"cw":0.51}}
```

Successful trainer responses report the safety-clamped command:

```json
{"type":"response","requestId":"erg1","success":true,"data":{"applied":{"operation":"set_target_power","value":250}}}
```

Operations are `request_control`, `reset`, `start`, `stop`, `set_resistance`,
`set_target_power`, and `set_simulation`. Parameterless operations omit `value`.
BLE replies require a successful write and matching Control Point indication.
For smoothed resistance, `applied` reports the accepted, quantized destination after
establishing the minimum baseline if needed; later ramp steps each require their
own acknowledgement. A response is not confirmation that the destination has been
reached. Later failures emit `error` and disconnect the device. Real telemetry does
not currently publish normalized resistance. Other commands report their confirmed,
quantized target. Device telemetry can still differ from commanded targets.

Default limits: 800 W, ±15% grade, resistance 0–0.7. Simulation wind: ±20 m/s;
`crr`: 0–0.02; `cw`: 0–1 kg/m. Resistance smoothing defaults to 0.2 units/second.
Finite excessive values are clamped; invalid numeric types, non-finite float values,
negative ERG watts, and ERG integers above 65535 are rejected. These rules also
apply to the BLE backend, which also intersects device ranges and quantizes to
supported increments. Upward BLE resistance ramps are rate limited; decreases
bypass smoothing at the next ramp tick. ERG and simulation changes are not ramped.

## Telemetry

```json
{"type":"telemetry","deviceId":"mock-trainer","data":{"powerWatts":243,"cadenceRpm":88.5,"speedKph":30.2,"heartRateBpm":142,"distanceMeters":120.0,"resistanceLevel":0.35,"timestampMs":1789776000000}}
```

MockTrainer emits four times per second while connected. All measurement fields are optional;
`timestampMs` is Unix milliseconds and always present. Absence means unavailable.
Power is signed to preserve future sensor data; heart rate is unsigned 16-bit.
Resistance is normalized. Distance increases with simulated speed. This is a
development signal generator, not a scientifically validated cycling model.

FTMS telemetry arrives at the trainer's rate, once per complete measurement record.
It uses the same envelope and optional `powerWatts`, `cadenceRpm`, `speedKph`,
`heartRateBpm`, and `distanceMeters` fields. It may also include
`averagePowerWatts` (signed watts) and `elapsedTimeSeconds` (unsigned seconds).
`timestampMs` is the daemon's receipt time for the final notification.
Absent values are omitted, not carried over from earlier measurements. Raw FTMS
resistance is not published as normalized `resistanceLevel`. Invalid packets are
dropped; `invalid_device_data` is emitted at most once every five seconds per device.
See [field layout and record assembly](ftms.md).

Cycling Power uses the same `telemetry` envelope with signed `powerWatts` and
optional `cadenceRpm` derived from crank revolution counters. It does not invent
speed or resistance. The initial crank sample establishes a baseline; unchanged
counts report zero cadence after three seconds of continued notifications.
Malformed packets use the same rate-limited `invalid_device_data` error policy.

Heart Rate Service monitors use `kind: "heart_rate_monitor"` and verified
`capabilities: ["heart_rate"]`. Their `telemetry` events contain `heartRateBpm`
(unsigned 16-bit) and `timestampMs`, under the monitor's own `deviceId`. When a
sensor supporting contact detection reports no/poor contact, a timestamp-only
sample clears BPM. Energy and RR fields are validated but not exposed. Malformed
notifications follow the same error throttling above. Trainer commands on these
sensors return `unsupported_operation`. Apps combine sensor and bike events with
independent freshness; the daemon does not merge identities. See [Heart Rate](heart-rate.md).

## Mock controls

Override any subset of measurements:

```json
{"type":"mock.setTelemetry","requestId":"m1","deviceId":"mock-trainer","data":{"powerWatts":300,"cadenceRpm":90,"speedKph":32,"heartRateBpm":150}}
```

Accepted ranges: power 0–2000 W; cadence 0–250 RPM; speed 0–150 km/h; heart rate
30–240 BPM. Invalid patches are rejected atomically. Injection changes measured
values only; it is not a physical trainer command. Omitted fields keep prior
overrides. Trainer load commands clear the power override; reset, owner disconnect,
device disconnect, and shutdown clear all overrides. Mock injection acquires the
same session ownership as trainer control. Stopped trainers continue to report
zero power, cadence, and speed until resumed.

Inject button edges individually (press does not automatically synthesize release):

```json
{"type":"mock.input","requestId":"i1","deviceId":"mock-controller","data":{"input":"shift_up","state":"pressed"}}
```

```json
{"type":"input","deviceId":"mock-controller","data":{"input":"shift_up","state":"pressed"},"timestampMs":1789776000000}
```

Digital inputs: `shift_up`, `shift_down`, `steering_left`, `steering_right`, `confirm`,
`back`, and `button`. They require `pressed` or `released` state and no `value`.
Generic `button` additionally requires unsigned 16-bit `button` index; other inputs
must omit it. `steering` requires `state: "value"` and a finite `value` from -1 to 1;
`gear` requires `state: "value"` and an integer `value` from 1 to 100.
`brake` requires `state: "value"` and a finite value from 0 to 2 (one fully applied
brake is 1; combined braking can reach 2). Generic `button` also accepts
`state: "value"` with a finite value from 0 to 255; the OpenBikeControl bridge uses
this for unmodified protocol analog bytes. The `button` index remains required.

## HTTP endpoints

| Method/path | Response |
| --- | --- |
| `GET /api/status` | Version, protocolVersion, uptimeSeconds, bluetoothEnabled, bluetoothAvailable, scan, mockMode, websocketClients, connectedDevices |
| `GET /api/adapters` | AdapterInfo array; empty in mock mode or when no adapters are available |
| `GET /api/devices` | Array of DeviceInfo |
| `GET /api/devices/{id}` | DeviceInfo or 404 with `type: error` and `data` error object |
| `POST /api/devices/{id}/connect` | Connect and return DeviceInfo or typed HTTP error |
| `POST /api/devices/{id}/disconnect` | Disconnect and return DeviceInfo or typed HTTP error |
| `POST /api/scan/start` | ScanStatus or typed HTTP error |
| `POST /api/scan/stop` | ScanStatus or typed HTTP error |

```json
{"version":"0.1.0","protocolVersion":1,"uptimeSeconds":12,"bluetoothEnabled":true,"bluetoothAvailable":true,"scan":{"scanning":true,"adapterId":"adapter-<opaque-uuid>","lastError":null},"mockMode":false,"websocketClients":1,"connectedDevices":0}
```

`bluetoothEnabled` means a native backend is attached. `bluetoothAvailable` means
the selected adapter was last observed not powered off (unknown radio state can
still be attempted); inspect `scan.lastError` for actual scan failures. Status and
device endpoints read cached state without waiting for Bluetooth.

HTTP trainer control is not implemented. Session-based WebSocket trainer control is
the supported interface. If HTTP trainer control is added later,
its ownership/lease lifecycle must be defined first.

## Errors

Current domain codes: `device_not_found`, `device_disconnected`, `connection_failed`, `invalid_device_data`,
`unsupported_operation`, `trainer_control_denied`, `trainer_control_failed`, `invalid_command`, `invalid_value`,
`events_lost`, `bluetooth_unavailable`, `adapter_not_found`, `scan_failed`, `timeout`,
and `busy`. `internal_error` remains reserved. HTTP access rejection uses `access_denied` with status 403.
Transport write/heartbeat timeout, excessive message size, and rate violations
close the session and release its control. Clients must tolerate disconnects.


## Recorder and passive replay

See [Recorder / Replay](recorder-replay.md) for the versioned `.biketrace` format,
`command.started` / `command.finished`, `session.disconnected`, and `replay.reset`.
The new event families are opt-in; existing telemetry subscriptions are unchanged.
`GET /api/replay` returns status or null. `POST /api/replay/start`, `/pause`, and
`/restart` control replay only. `/api/status` includes an optional `replay` object.

Replay preserves original events and timestamps without constructing Bluetooth
transports. Incoming trainer commands are rejected; recorded commands are emitted
as events. Device snapshots and connection changes follow the trace. After restart,
replace local device state with the snapshot in `replay.reset` or fetch `/api/devices`.


## OpenBikeControl inputs

A discovered OpenBikeControl BLE bridge is a `bike_controller`. Connect it with
`device.connect` or the HTTP/CLI connection command. A verified notification
channel advertises `controller_input`. Its normalized input events use the same
subscription, recording, and replay path as mock inputs. Trainer commands on a
controller return `unsupported_operation`.

Connections are process-wide; viewer exit does not close them. Explicit disconnect
cancels retries. Unexpected link loss emits releases for held inputs, a disconnect,
and up to five `device.reconnecting` attempts. Protocol errors close the session
without retry. Read [Zwift controller integration](zwift-controllers.md) for mappings,
BikeControl setup, aggregate identity, and hardware validation limits.

## Direct Click V2 inputs

Identified Click V2 devices use `kind: "bike_controller"` and gain
`controller_input` after GATT and handshake verification. They emit the same
`input` envelope as the OpenBikeControl bridge. See [button mappings, setup, and
firmware limitations](zwift-click-v2.md). Discovery does not depend on display
names and does not expose raw manufacturer data. Automatic retries never resend
trainer commands; controller inputs never acquire a trainer-control lease.
