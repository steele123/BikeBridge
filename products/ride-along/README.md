# Ride Along

A small Svelte 5 + Tauri 2 desktop window for riding while watching a video,
working, or browsing. It starts pinned above other windows and consumes the
BikeBridge public API on the same computer.

## Run it

Prerequisites: Rust, Bun, Node.js 22.12+ (or 24+), and the native
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.
On macOS, install Xcode Command Line Tools.

Start the daemon from the repository root:

```sh
cargo run -p bikebridge-cli -- run
```

Connect your bike in the [dashboard](http://127.0.0.1:9376), then in another terminal:

```sh
cd products/ride-along
bun install --frozen-lockfile
bun run desktop
```

The app automatically selects a connected power source. Open **Settings** to pick
another discovered bike, connect it, or change the local API port. The chosen name
and port are remembered. After a daemon restart, a remembered name is selected
only when exactly one device has that name. Discover new Bluetooth names in the
dashboard; this product does not run its own Bluetooth scan.

### Window controls

- Drag the title to place the window beside your video.
- The pin button toggles **always on top**; it starts enabled on every launch.
- Compact mode reduces the window to watts, cadence, speed, and a timer.
- Settings opens the source selector and connection setup.
- Minimize and close use the native window controls.

On macOS the window joins all desktops and is marked as a fullscreen auxiliary
window. Fullscreen video behavior can vary by player and OS; ordinary windowed
video is the baseline supported use. Fullscreen playback has not been verified.

### Ride readings

- Watts, cadence, speed, and optional heart rate come from the selected device.
- Missing readings and readings older than five seconds display a dash. Fields
  omitted from the newest measurement are immediately unavailable.
- The graph shows the last minute of power, sampled once per second. Gaps stay
  empty. Negative power is preserved numerically and clipped at zero in the graph.
- **Ride / Pause / Reset** manage a local timer and average only. The timer runs
  until paused, including any connection gaps. Average power is weighted by time,
  excludes paused time and unavailable readings, and includes actual zero watts.
- Session statistics reset when the app closes. This is not a workout recorder.
- No resistance, ERG, simulation, or trainer ownership commands are issued.
- Mock and replay sources are labeled **DEMO** and **REPLAY**.

BikeBridge must remain running. The app reconnects automatically when the daemon
returns. It does not bundle or start a daemon in this first version.

## Build and verify

```sh
bun run check
bun test
bun run package
cargo test --release --manifest-path src-tauri/Cargo.toml
```

The default bundle target is a macOS app:

```text
src-tauri/target/release/bundle/macos/Ride Along.app
```

You can launch that app directly or copy it into Applications. Distribution signing,
notarization, and installers are not configured. Windows/Linux builds have not been
verified; build on the target OS and override `--bundles` for its package format.

The Tauri crate has its own Cargo workspace and lockfile, so root `cargo test
--workspace` does not pull in desktop GUI dependencies. Built frontend files and
native binaries are ignored by Git; build them with the commands above.

For browser-only UI development, `bun run dev` serves port 1420 and proxies API
traffic to port 9376. Native window controls are disabled in the browser preview.
The preview's proxy port is configured in `vite.config.js`.

## API boundary

The native shell exposes only fixed local operations: status, device inventory,
explicit device connection, and an event subscription. Addresses are restricted
to `127.0.0.1` with a validated port; device IDs cannot inject paths. HTTP redirects
and environment proxies are disabled. WebSocket events use a Tauri IPC channel.
The existing daemon browser-origin checks remain in place.

The frontend never imports dashboard internals. If another product needs the same
client, extract a shared API package at that point.
