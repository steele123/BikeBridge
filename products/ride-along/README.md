# Ride Along

A small Svelte 5 + Tauri 2 desktop window for riding while watching a video,
working, or browsing. It starts pinned above other windows and consumes the
BikeBridge public API on the same computer.

## Run it

Prerequisites: Rust, Bun, Node.js 22.12+ (or 24+), and the native
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.
On macOS, install Xcode Command Line Tools.

Start the desktop app (its Bluetooth service starts automatically):

```sh
bun install --frozen-lockfile --cwd products/dashboard
bun install --frozen-lockfile --cwd products/stream-overlay
cd products/ride-along
bun install --frozen-lockfile
bun run desktop
```

Choose **Devices & dashboard** in Settings to connect your bike. Choose
**Stream overlay** to set up an OBS Browser Source. The same shortcuts are in the
tray menu. See [desktop packaging](../../docs/desktop-bundle.md) for the single
BikeBridge installer and distribution status.

The app automatically selects a connected power source. Open **Settings** to pick
another discovered bike, connect it, or change the local API port. The chosen name
and port are remembered. After a daemon restart, a remembered name is selected
only when exactly one device has that name. Discover new Bluetooth names in the
dashboard; this product does not run its own Bluetooth scan.

### Separate heart-rate monitor

Connect your BLE chest strap or armband in the dashboard. In Ride Along settings,
choose it under **Heart-rate source**, or use **Connect monitor** if it is discovered
but disconnected. The selection is remembered. Choose **Use bike heart rate** to
restore the bike's embedded reading. A selected monitor that loses contact,
disconnects, or stops sending data shows a dash; bike updates do not refresh it.
Heart rate is visible in expanded and compact mode.

### Window controls

- Drag the title to place the window beside your video.
- The pin button toggles **always on top**; it starts enabled on every launch.
- Compact mode reduces the window to watts, cadence, speed, and a timer.
- Settings opens the source selector and connection setup.
- Close hides the window to the tray so your overlay stays connected.
- Choose **Quit BikeBridge** in the tray to stop the app and its included service.

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
- Session statistics reset when the app quits. This is not a workout recorder.
- No resistance, ERG, simulation, or trainer ownership commands are issued.
- Mock and replay sources are labeled **DEMO** and **REPLAY**.

The included service stays running while the window is hidden. If a compatible CLI
daemon already exists, the app reuses it and leaves it running when the app quits.

## Build and verify

```sh
bun run check
bun test
bun run package
cargo test --release --manifest-path src-tauri/Cargo.toml
```

The installer is named **BikeBridge** and contains all products. On macOS, use
`bun run package -- --bundles app,dmg`; on Windows use `--bundles nsis`. Build output is
under `src-tauri/target/release/bundle/`. Public distribution still requires signing
and platform installation tests; see the [packaging guide](../../docs/desktop-bundle.md).

The Tauri crate has its own Cargo workspace and lockfile, so root `cargo test
--workspace` does not pull in desktop GUI dependencies. Built frontend files and
native binaries are ignored by Git; build them with the commands above.

For browser-only UI development, `bun run dev` serves port 1420 and proxies API
traffic to port 9376. Native window controls are disabled in the browser preview.
The preview's proxy port is configured in `vite.config.js`.

## API boundary

The native shell exposes only fixed local operations: status, device inventory,
explicit device connection, service startup, fixed product shortcuts, and an event subscription. Addresses are restricted
to `127.0.0.1` with a validated port; device IDs cannot inject paths. HTTP redirects
and environment proxies are disabled. WebSocket events use a Tauri IPC channel.
The existing daemon browser-origin checks remain in place.

Shared heart-rate source selection and freshness live in `../shared/heart-rate.ts`.
The frontend never imports dashboard internals. If another product needs the same
client, extract a shared API package at that point.
