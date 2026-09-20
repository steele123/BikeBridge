# Heart-rate monitors

BikeBridge supports standard Bluetooth LE Heart Rate Service (`0x180D`) monitors,
such as chest straps and armbands that broadcast the notifying Heart Rate
Measurement characteristic (`0x2A37`). Model-specific hardware testing is pending.
ANT+ and proprietary smartwatch heart-rate protocols are not implemented.

## Connect

1. Wear/wake the monitor and enable its Bluetooth heart-rate broadcast mode if needed.
2. Start BikeBridge with `cargo run -p bikebridge-cli -- run`.
3. In the dashboard, select the discovered heart-rate monitor and click **Connect**.
   Use **Browse Bluetooth** or **Find by name** if it does not advertise its service.
4. Select the monitor to view its live BPM. For combined bike and heart-rate metrics,
   choose **Heart-rate source** in Ride Along or stream overlay setup.

If connection fails, check whether a phone or another app has already used the
monitor's available Bluetooth connections. Not all monitors support multiple clients.

The normal CLI works too:

```sh
cargo run -p bikebridge-cli -- connect --name "Your monitor name"
```

## Data behavior

The decoder implements the [Bluetooth SIG Heart Rate Service 1.0 measurement format](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/HRS_v1.0/out/en/index-en.html):
8-bit and little-endian 16-bit rates, contact flags, optional energy expended,
and one or more optional RR intervals. Truncated fields, reserved flags, odd RR
lengths, and unexpected bytes are rejected. Energy and RR values are validated
but are not published by the current API.

The monitor publishes ordinary `telemetry` events under its own device ID:

```json
{"type":"telemetry","deviceId":"ble-…","data":{"heartRateBpm":142,"timestampMs":1789776000000}}
```

When supported contact detection reports no/poor contact, `heartRateBpm` is omitted
in a new timestamped sample, clearing the previous rate. Monitors without contact
detection retain their reported BPM, including zero. Invalid packets are dropped;
`invalid_device_data` errors are emitted at most once per five seconds per device.

Discovery alone does not grant capabilities or connect automatically. Successful
subscription verifies `heart_rate_monitor` with only the `heart_rate` capability.
No readable feature characteristic or control point is required. On devices with
multiple supported measurement services, FTMS is preferred, then Cycling Power,
then Heart Rate. Heart-rate sessions never expose trainer load controls.

Device sessions are shared by clients. Closing an overlay does not disconnect the
monitor. Explicit disconnect/shutdown tears down the session. The existing optional
`[trainer].auto_reconnect` policy also applies to BLE heart-rate sessions.

Ride Along and the stream overlay track monitor freshness independently of bike
telemetry and clear it after five seconds. A selected monitor going offline does
not silently substitute another device or the bike's embedded rate.

## Try without hardware

The hand-authored synthetic trace has a bike reporting 100 BPM and a separate strap
reporting 140–149 BPM, so source selection is easy to verify. It includes strap
disconnect at 20s, reconnect at 25s, contact loss at 40–41s, and no strap readings
at 50–56s while the bike continues. It is not a recording from real hardware.

```sh
cargo run -p bikebridge-cli -- --port 9380 replay examples/traces/heart-rate-ride.biketrace
```

Open `http://127.0.0.1:9380/overlay/`, select **Demo Bike** and **Demo Chest Strap**,
then start the paused replay:

```sh
cargo run -p bikebridge-cli -- --port 9380 replay-control start
```

Use `replay-control restart` to repeat. For Ride Along, set port **9380** in settings
and select the same sources. Return to port **9376** afterward for your live daemon.
