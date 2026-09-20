# BikeBridge

One desktop app for your bike, heart-rate monitor, ride companion, and stream
overlay. BikeBridge handles Bluetooth and shares live readings through a local
HTTP/WebSocket API.

**Your bike → BikeBridge → your apps**

## Desktop setup

The **BikeBridge installer** includes the Bluetooth service, dashboard, Ride Along,
and OBS stream overlay. The service starts when you open the app—no terminal or
separate installation is needed.

1. Install and open **BikeBridge**.
2. Choose **Devices & dashboard**, wake your bike, and click **Connect**.
   Use **Browse Bluetooth** or **Find by name** if it doesn't appear.
3. Open **Ride Along** for an always-on-top view, or **Stream overlay** for OBS.
4. For streaming, customize the overlay, copy its URL, and add it as an OBS
   **Browser Source** using the dimensions shown in setup.

Closing the Ride Along window hides it to the tray/menu bar; Bluetooth and the
overlay keep running. Use the tray to reopen a product or **Quit BikeBridge**.
Keep BikeBridge open while riding or streaming, with OBS on the same computer.

**Availability:** the Apple Silicon Mac test installer has been built and tested.
Windows installer automation is included; Windows testing and release signing
are still pending. Public signed downloads are not yet available.
See [installer build instructions and packaging details](docs/desktop-bundle.md).

## Run from source (CLI)

Install stable Rust and your platform's build tools. Linux also needs
`libdbus-1-dev` and `pkg-config`. See [platform setup](docs/devices.md).

From the repository root, start BikeBridge:

```sh
cargo run -p bikebridge-cli -- run
```

Open the [dashboard](http://127.0.0.1:9376), wake your bike, and click **Connect**.
If it doesn't appear, use **Browse Bluetooth** or **Find by name**.

To try it without hardware, start with mock devices instead:

```sh
cargo run -p bikebridge-cli -- run --mock
```

Keep the daemon running while using your apps. Stop it with **Ctrl+C**.

## Apps

| App | What it does | Open / setup |
| --- | --- | --- |
| Dashboard | Find devices, view telemetry, control supported trainers, and explore the API. | [Open dashboard](http://127.0.0.1:9376) · [Docs](products/dashboard/README.md) |
| Ride Along | An always-on-top desktop window for riding while watching videos or working. | [Setup](products/ride-along/README.md) |
| Stream overlay | Transparent cycling stats for OBS, with customizable layouts and metrics. | [Open setup](http://127.0.0.1:9376/overlay/) · [Docs](products/stream-overlay/README.md) |

The desktop app bundles all three products and starts the service automatically.
The CLI also includes the dashboard and stream overlay.

## Device support

- **FTMS trainers:** live telemetry and supported resistance, ERG, and simulation controls.
- **Bluetooth heart-rate monitors:** standalone BPM, usable alongside your bike in Ride Along and the stream overlay.
- **Cycling Power sensors:** watts and cadence when supplied by the sensor.
- **Zwift Click V2:** experimental direct button input; firmware limitations apply.
- **Other controllers:** supported Zwift, Di2, and AXS inputs through the BikeControl bridge.
- **Mock devices:** simulated trainer and controller for development without hardware.

Support depends on the device's services and firmware. Version 0.1.0 is still in
development; physical trainer control and controller compatibility need further
validation. See [device support](docs/devices.md) and [Click V2 setup](docs/zwift-click-v2.md).

## Build with the API

- HTTP: `http://127.0.0.1:9376/api`
- WebSocket: `ws://127.0.0.1:9376/ws`
- [API reference](docs/protocol.md)
- [JavaScript examples](examples/javascript/) · [Rust example](examples/rust-console/)

Apps can subscribe to telemetry and controller inputs, connect devices, and issue
supported trainer commands. [Record and replay](docs/recorder-replay.md) sessions
to reproduce events without hardware. Packaged TypeScript, C#, and Unity SDKs are planned.

BikeBridge runs on the local computer only. Browser clients must use the daemon's
own origin. Trainer control has one owner at a time; see [control behavior and
limits](docs/ftms-control.md).

## Configuration

The desktop app uses built-in defaults. For the CLI, copy
[bikebridge.example.toml](bikebridge.example.toml) to `bikebridge.toml` to configure
Bluetooth names, scanning, reconnection, and trainer limits. If a compatible CLI
service is already running, the desktop app reuses it and leaves it running on exit.

For CLI commands and options:

```sh
cargo run -p bikebridge-cli -- --help
```

## Development

```sh
cargo test --workspace --locked
cargo build --release --locked -p bikebridge-cli
```

The executable is `target/release/bikebridge` (`bikebridge.exe` on Windows).

- [`crates/`](crates/): Rust daemon, protocols, and hardware integrations.
- [`products/`](products/README.md): dashboard, desktop companion, and stream overlay.
- [`examples/`](examples/): API clients and sample traces.
- [`docs/`](docs/): architecture, protocols, and device guides.

Frontend setup and build commands are in each product's README. Rebuild the daemon
after changing embedded dashboard or overlay assets. See [architecture](docs/architecture.md)
for the internal design.

## License

[MIT](LICENSE)
