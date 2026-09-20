# One download: BikeBridge desktop

The desktop app includes the Bluetooth service, device dashboard, OBS stream
overlay, and Ride Along. Users install **BikeBridge**, open it, connect their
devices, and copy the overlay URL into OBS. No terminal, Rust, Bun, or separate
service installation is needed on the user's computer.

## Everyday use

1. Open BikeBridge. The included service starts automatically on port 9376.
2. Choose **Devices & dashboard** and connect the bike and optional monitor.
3. Choose **Stream overlay**, customize it, and copy its Browser Source URL to OBS.
4. Hide the Ride Along window if it is not needed. BikeBridge stays in the system
   tray/menu bar, keeping the overlay and device connections alive.
5. Choose **Quit BikeBridge** from the tray when finished. This stops the included
   service and disconnects its devices. OBS will show unavailable readings.

The tray can reopen Ride Along, the dashboard, or overlay setup. Launching a
second copy brings back the existing app. BikeBridge must stay open while riding
or streaming; automatic launch at login is not enabled.

## Architecture

The Tauri shell currently lives in `products/ride-along/src-tauri`; its installer
is named BikeBridge. Ride Along remains the companion window. The shell links
the existing Rust server and Bluetooth crates directly into its executable.
The dashboard and overlay builds are embedded in that server. Future products
can join this shell without another Bluetooth connection or installation.

This avoids a separate executable, process supervision, and orphaned services.
All products still consume the same public loopback HTTP/WebSocket API. Existing
origin checks, trainer safeguards, and explicit device connection remain active.
Normal shutdown runs the server's safe-state and Bluetooth cleanup.

If a compatible CLI daemon already occupies port 9376, the app uses it and leaves
it running on exit. An unrelated service on that port produces a visible error
and a Retry action; the app never silently changes ports and breaks OBS URLs.
Advanced connection settings can point Ride Along at a separate local service.
The embedded service uses the built-in default safety settings and does not read
an incidental `bikebridge.toml` from the launch directory. Use the CLI for custom
daemon configuration, recording, and replay.

## Build an installer

Developers need Rust, Bun, Node.js 22.12+ (or 24+), and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
Install dependencies for all three frontends once:

```sh
bun install --frozen-lockfile --cwd products/dashboard
bun install --frozen-lockfile --cwd products/stream-overlay
bun install --frozen-lockfile --cwd products/ride-along
cd products/ride-along
bun run package -- --bundles app,dmg -- --locked
```

On Windows, use `--bundles nsis` instead. The Windows installer installs WebView2
if needed (that step requires internet). The macOS bundle includes Bluetooth
permission descriptions. The build hook rebuilds all three frontends before
compiling the executable, so no separately hosted assets are needed.

Outputs are under `products/ride-along/src-tauri/target/release/bundle/`:

- macOS: `dmg/BikeBridge_*.dmg` and `macos/BikeBridge.app`.
- Windows: `nsis/BikeBridge_*-setup.exe`.

The manual **Desktop installers** GitHub Actions workflow builds macOS and Windows
test installers and uploads artifacts. It does not publish a release. Each build
targets its runner's architecture; this is not yet a universal macOS installer.
Linux can be built with Tauri's native prerequisites, but has no installer job yet.

## Distribution status

If macOS packaging reports an unsupported `C.UTF-8` locale, rerun with
`LC_ALL=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 LANG=en_US.UTF-8` set for that command.

Local builds are suitable for development and testing. Before distributing to
streamers, configure [macOS signing and notarization](https://v2.tauri.app/distribute/sign/macos/)
and [Windows signing](https://v2.tauri.app/distribute/sign/windows/), run installation
and Bluetooth tests on each target OS, then publish the installers in a release.
Unsigned downloads may be blocked or warned about by the OS. Signing credentials,
release publication, and automatic updates are not configured here.
