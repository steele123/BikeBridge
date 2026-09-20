# BikeBridge Stream Overlay

A Svelte overlay for OBS and other streaming software with a Browser Source.
BikeBridge embeds its compiled assets, so normal use needs no Node.js or separate
frontend server.

## Use in OBS

1. Run `cargo run -p bikebridge-cli -- run` from the repository root.
2. Connect your bike in the [dashboard](http://127.0.0.1:9376).
3. Open **Stream overlay** in the dashboard, or visit
   [the setup page](http://127.0.0.1:9376/overlay/).
4. Choose a source, layout, metrics, theme, accent, and speed unit.
5. Click **Copy overlay URL**. In OBS add a **Browser** source with **Local file**
   disabled, paste the URL, and use the dimensions shown in setup:
   horizontal **960 × 260**, stacked **360 × 620**.
6. Position or scale the source within your scene. Keep BikeBridge running.

See the [official OBS Browser Source guide](https://obsproject.com/kb/browser-source)
for its URL, viewport, and source lifecycle settings. No custom CSS is required;
the page background is transparent and the metric panel is translucent. This app
does not require OBS control permissions.

The setup page and broadcast view are separate modes. The broadcast URL includes
`view=overlay` and renders only the overlay. Changes made in setup are encoded in
the generated URL: paste the new URL into OBS after customization. Bookmark the
configured URL without `view=overlay` to return to its setup. Sample data is only
a setup preview; it is never enabled by the generated broadcast URL.

OBS and BikeBridge must run on the same computer. Loopback browser-origin checks
remain enabled. Remote streaming PCs and hosted browser overlays are not supported.

## Telemetry behavior

- Power, cadence, speed, and heart rate are optional display fields. Values absent
  from the newest measurement display a dash. Nothing is fabricated for sensors
  that do not supply speed or heart rate.
- Values expire after five seconds without a reading. Disconnects clear readings
  immediately, and the view reconnects automatically when BikeBridge returns.
- The graph covers the last 60 seconds using local receipt time. Gaps stay empty.
  Signed power is displayed numerically; negative graph values are clipped at zero.
- Mock and replay telemetry are marked **DEMO** and **REPLAY**.
- An explicit source uses its device ID while available, then falls back to its
  exact display name after a daemon restart only if that name is unique. If the
  name is ambiguous, choose the intended device again. Automatic source selection
  uses a connected device advertising power capability.
- This client only reads inventory/status and subscribes to events. It does not
  scan, connect devices, claim trainer control, or change resistance.

## Development

Requires Bun and Node.js 22.12+ or 24+.

```sh
cd products/stream-overlay
bun install --frozen-lockfile
bun run dev
```

Open `http://127.0.0.1:1421/overlay/`. The dev server proxies `/api` and `/ws` to
BikeBridge on port 9376. Production uses the daemon's own origin and port.

```sh
bun run check
bun test
bun run build
cd ../..
cargo test -p bikebridge-server --test mock_api dashboard_assets_and_exact_same_origin_requests
cargo build -p bikebridge-cli
```

Restart BikeBridge after rebuilding embedded assets. Check in `dist/index.html`,
`dist/app.js`, and `dist/app.css` alongside source changes; Rust-only builds and CI
use these files directly. The static routes reuse the dashboard's security headers.

Verification covers configuration/source selection, missing and stale readings,
HTTP asset routes and origin protection, plus browser rendering at OBS dimensions.
Actual capture inside OBS has not been tested yet.
