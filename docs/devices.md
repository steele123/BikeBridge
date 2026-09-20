# Device support

## Phase 1 (implemented)

| Device | ID | Capabilities |
| --- | --- | --- |
| MockTrainer | `mock-trainer` | Power, cadence, speed, embedded heart rate, distance, resistance, ERG, simulation |
| MockController | `mock-controller` | Normalized buttons, gear values, steering |

Enable both with `bikebridge run --mock`. Both initially connect; they can be
disconnected/reconnected through WebSocket. MockTrainer emits four samples per
second, with deterministic small variations, while MockController emits only
explicitly injected inputs. Multiple clients can observe both simultaneously.

MockTrainer implements the core `Trainer` trait. Safety ceilings apply to load
commands, while `mock.setTelemetry` supplies development measurements only.
Mock heart rate comes from the trainer. A [synthetic two-device replay](heart-rate.md#try-without-hardware) demonstrates a separate monitor.

## Phase 2 (implemented: BLE discovery)

`bikebridge-ble` uses btleplug to enumerate adapters, choose a default powered-on
radio, scan for cycling services, and publish normalized discovery events.
Scanning does not connect devices. The daemon starts without a working radio
and exposes failures through `status.scan.lastError` and scan API error responses.

| Advertised service | Service UUID | Provisional device kind |
| --- | --- | --- |
| Fitness Machine | `0x1826` | `trainer` |
| OpenBikeControl | `d273f680-d548-419d-b9d1-fa0472345229` | `bike_controller` |
| Cycling Power | `0x1818` | `power_meter` |
| Cycling Speed and Cadence | `0x1816` | `cadence_sensor` |
| Heart Rate | `0x180D` | `heart_rate_monitor` |

The four standard UUIDs were checked against [Bluetooth SIG Assigned Numbers](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Assigned_Numbers/out/en/index-en.html).
The OpenBikeControl UUID comes from its [BLE specification](https://github.com/OpenBikeControl/openbikecontrol-protocol/blob/c057e1d7ad05ceb1a6d59a0ca95e57394b7f9b48/BLE.md).
When a device advertises multiple listed services, primary role precedence follows
the table order. All recognized service indications are merged internally across
partial advertisements. Names alone are never used to infer compatibility.

Role detection is provisional. Fitness Machine can describe equipment other than
an indoor bike, and CSC may be wheel-only or crank-only. Every BLE discovery has an
empty capability list until connection/GATT feature verification. Detection
does not imply support for ERG, resistance, simulation, or any measurement field.

Click V2 controllers are identified separately by Zwift manufacturer ID `0x094a`
and model `0x0a`/`0x0b`. Their shared proprietary service alone is insufficient.
See [direct Click V2 support](zwift-click-v2.md).

The application scans without OS service filters and post-filters candidates. Some devices
do not advertise standard services, advertise only while awake/unpaired, or stop
advertising while connected to another app. Those may not appear. Unrecognized proprietary devices need explicit name selection; unrelated nearby devices are not listed.

IDs are random BikeBridge UUIDs retained for the daemon session. BLE addresses and
platform IDs remain private; there is no cross-restart identity persistence yet.
Scan stop preserves cached records. A listed device is not guaranteed still nearby.
Platform adapter labels are generic; enumeration order supplies the optional
zero-based adapter selection index. Multi-adapter discovery can produce separate
records for the same physical peripheral; cross-adapter identity merging is deferred.

## Platform setup

- **Windows:** use an enabled Bluetooth LE radio and ensure Windows allows Bluetooth
  access. Start `bikebridge run --no-auto-scan`, inspect `bikebridge adapters`, then
  use `bikebridge scan`. The implementation uses WinRT through btleplug.
- **Linux:** build with `libdbus-1-dev` and `pkg-config` installed (Debian/Ubuntu:
  `sudo apt-get install libdbus-1-dev pkg-config`). At runtime, BlueZ and the system
  D-Bus service must be available, with permission to scan. Check radio power and
  rfkill if discovery fails. The executable still needs the native D-Bus runtime.
- **macOS:** grant Bluetooth permission when prompted. The CLI embeds an
  `NSBluetoothAlwaysUsageDescription` in its executable for `cargo run` and
  standalone builds. App bundling is not included yet. See btleplug's
  [platform installation notes](https://github.com/deviceplug/btleplug#buildinstallation-notes-for-specific-platforms).

`run --mock` never initializes the native backend and requires no radio or runtime
Bluetooth permission. The normal build still includes the platform libraries.

On the local Windows x64 host, native adapter enumeration and scan start/stop were
successfully exercised. No cycling peripheral advertised during that check, so
physical-device classification remains unverified locally. Injected backend tests
cover the standard service types and OpenBikeControl, identity/metadata behavior, lifecycle failures, and
HTTP/WebSocket discovery. Linux/macOS CI is configured but has not been run here.

## Phase 3 (implemented: FTMS telemetry)

### Heart-rate monitors

Standard BLE Heart Rate Service monitors can connect and emit `heartRateBpm` independently of a trainer. See [heart-rate setup, data handling, and replay demo](heart-rate.md).

### Manual selection and Cycling Power

Use `run --device-name "Steele's Bike"` (repeatable), or `[bluetooth] device_names`
in the config, for a peripheral that omits cycling services from advertisements.
The scan includes matching OS/advertised names as unconnected, unclassified
candidates. Select it with `connect --name "Steele's Bike"`; duplicate names require
an ID. Case and curly apostrophes are normalized, but matching is not a substring
search. The original ID-based commands remain supported.

Connection first prefers readable FTMS features with notifying Indoor Bike Data.
Otherwise it verifies readable Cycling Power Feature (`0x2A65`) and notifying
Cycling Power Measurement (`0x2A63`) under Cycling Power Service (`0x1818`). Only
then does a candidate become a `power_meter` with `power` and optional `cadence`
capabilities. Other profiles are rejected and disconnected.

Cycling Power cadence requires two crank samples; 16-bit counter wrap is handled.
Unchanged crank counts report zero after three seconds of continued notifications.
Missing data, counter resets, implausible cadence above 300 RPM, and intervals of
64 seconds or more omit cadence until a fresh baseline is available. Reconnects
reset the baseline. Speed and trainer control are not exposed by this driver.

`node examples/javascript/trainer.mjs --name "Steele's Bike"` connects and prints
measurements. The XDS-T901-0204 on macOS was inspected and delivered standard
Cycling Power packets at rest; nonzero pedaling measurements remain unverified.

### FTMS connection requirements

Use `bikebridge connect <device-id>` or the Node `trainer.mjs` example to connect a
discovered FTMS candidate. The daemon requires readable Fitness Machine Feature
(`0x2ACC`) and notifying Indoor Bike Data (`0x2AD2`) under Fitness Machine Service.
It rejects other exercise-machine types without Indoor Bike Data. Advertised names
and manufacturer names do not bypass these checks.

The decoder publishes available power/cadence/speed and selected optional metrics.
Measurement capabilities come from the feature characteristic; Phase 4 separately
validates control capabilities. [Decoder layout and limitations](ftms.md) document the verified SIG
format and strict packet checks, including the uint8 resistance field layout.

Lost links and notification-stream closure disconnect the session and emit events.
Explicit disconnect and shutdown attempt bounded teardown. Retry using `connect`,
or enable the optional bounded automatic reconnect policy described in the protocol.

The complete bytes-to-WebSocket path is tested with an injected transport. No
physical trainer has supplied telemetry in this environment; compatibility with
individual trainer models is unverified. To validate yours, wake it, make it
available for a BLE connection, then run:

```sh
bikebridge run
# In another terminal:
bikebridge devices
node examples/javascript/trainer.mjs ble-<your-device-id>
```

Pedal and compare watts/RPM/speed with the trainer's display. Power off the trainer
to check `device.disconnected`; wake it and rerun the example to check reconnection.
Stop with `bikebridge disconnect <device-id>` or Ctrl+C in the daemon terminal.
Record the model, firmware, OS, observed fields, and any typed errors when reporting
compatibility. No raw Bluetooth identifiers are needed.

## Phase 4 (implemented: FTMS control)

The Control Point (`0x2AD9`, WRITE/INDICATE) and Machine Status (`0x2ADA`, NOTIFY)
are required for control. Target-setting features gate resistance, ERG, and indoor
bike simulation. Resistance and ERG also require valid six-byte supported ranges
(`0x2AD6`, `0x2AD8`). Unsupported modes remain unavailable while telemetry continues.
Three-byte resistance ranges are deliberately unsupported; see the documented
[SIG errata and wire-format choices](ftms-control.md).

Single-client ownership, acknowledged writes, configured ceilings, resistance
ramps, cancellation, best-effort Stop/Reset, and bounded reconnects are exercised
with injected transports. Run `node examples/javascript/control.mjs <device-id>`
for interactive control. Complete the [physical acceptance checks](ftms-control.md#hardware-acceptance)
before claiming compatibility for a trainer model or relying on cleanup behavior.

## OpenBikeControl controller bridge (implemented)

Click, Play, Ride, Shimano Di2, and SRAM AXS inputs can reach BikeBridge through BikeControl's
OpenBikeControl BLE bridge on a phone or second computer. The new
`bikebridge-openbikecontrol` crate owns decoding, button state, connection workers,
release-on-disconnect, and bounded retries; native GATT remains in `bikebridge-ble`.
See [Zwift controller setup](zwift-controllers.md) and [Di2 / AXS setup](di2-axs.md).
Di2 needs D-Fly assignments; AXS needs BikeControl 6.3+ for individual buttons and
its setup/restore workflow. The same bridge driver handles their mapped actions.

Connect explicitly using the ordinary device commands. The connected bridge has
`controller_input` capability and publishes normalized `input` events, which the
recorder captures without special handling. Bridge identity aggregates its physical
controllers; it does not identify each physical button source. No physical hardware
has been validated here. Other native proprietary Zwift/Di2/AXS pairing, network OpenBikeControl,
and certification are not implemented.
