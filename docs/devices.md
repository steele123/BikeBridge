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
Heart rate currently comes from the trainer; there is no separate mock HR monitor.

## Phase 2 (implemented: BLE discovery)

`bikebridge-ble` uses btleplug to enumerate adapters, choose a default powered-on
radio, scan for cycling services, and publish normalized discovery events.
Scanning does not connect devices. The daemon starts without a working radio
and exposes failures through `status.scan.lastError` and scan API error responses.

| Advertised service | SIG UUID | Provisional device kind |
| --- | --- | --- |
| Fitness Machine | `0x1826` | `trainer` |
| Cycling Power | `0x1818` | `power_meter` |
| Cycling Speed and Cadence | `0x1816` | `cadence_sensor` |
| Heart Rate | `0x180D` | `heart_rate_monitor` |

These UUIDs were checked against [Bluetooth SIG Assigned Numbers](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Assigned_Numbers/out/en/index-en.html).
When a device advertises multiple listed services, primary role precedence follows
the table order. All recognized service indications are merged internally across
partial advertisements. Names alone are never used to infer compatibility.

Role detection is provisional. Fitness Machine can describe equipment other than
an indoor bike, and CSC may be wheel-only or crank-only. Every BLE discovery has an
empty capability list until connection/GATT feature verification. Detection
does not imply support for ERG, resistance, simulation, or any measurement field.

The application applies both an OS service filter and post-filtering. Some devices
do not advertise standard services, advertise only while awake/unpaired, or stop
advertising while connected to another app. Those may not appear. Devices exposing
only proprietary services, and unrelated nearby devices, are not listed.

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
- **macOS:** grant Bluetooth permission to the terminal launching the executable.
  Distribution as an app bundle needs an `NSBluetoothAlwaysUsageDescription` in
  its Info.plist. App bundling is not included yet. See btleplug's
  [platform installation notes](https://github.com/deviceplug/btleplug#buildinstallation-notes-for-specific-platforms).

`run --mock` never initializes the native backend and requires no radio or runtime
Bluetooth permission. The normal build still includes the platform libraries.

On the local Windows x64 host, native adapter enumeration and scan start/stop were
successfully exercised. No cycling peripheral advertised during that check, so
physical-device classification remains unverified locally. Injected backend tests
cover the four service types, identity/metadata behavior, lifecycle failures, and
HTTP/WebSocket discovery. Linux/macOS CI is configured but has not been run here.

## Phase 3 (implemented: FTMS telemetry)

Use `bikebridge connect <device-id>` or the Node `trainer.mjs` example to connect a
discovered FTMS candidate. The daemon requires readable Fitness Machine Feature
(`0x2ACC`) and notifying Indoor Bike Data (`0x2AD2`) under Fitness Machine Service.
It rejects other exercise-machine types without Indoor Bike Data. Advertised names
and manufacturer names do not bypass these checks.

The decoder publishes available power/cadence/speed and selected optional metrics.
Measurement capabilities come from the feature characteristic; control capabilities
remain absent. [Decoder layout and limitations](ftms.md) document the verified SIG
format and strict packet checks, including the uint8 resistance field layout.

Lost links and notification-stream closure disconnect the session and emit events.
Explicit disconnect and shutdown attempt bounded OS teardown. Retry using `connect`;
automatic reconnection is deferred to Phase 4. Feature/range checks for control,
control-point transactions, and safe load commands also belong to Phase 4.

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

## Later controller support

OpenBikeControl integration is optional and belongs in its own crate after its
actual network protocol is verified. Direct proprietary Zwift BLE behavior is not
required. Inputs already use a hardware-independent vocabulary so later controllers
can publish the same event shape. No compatibility with Zwift Click, Play, Ride,
Di2, AXS, or any physical trainer is claimed by this phase.
