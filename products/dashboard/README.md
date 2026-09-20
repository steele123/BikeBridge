# BikeBridge dashboard

A small Svelte 5 + TypeScript app served by BikeBridge at
<http://127.0.0.1:9376>. It provides device discovery and connection, manual name
selection, live telemetry and a power graph, supported trainer controls, and an
HTTP/WebSocket console with exportable event logs.

**Browse Bluetooth** lists cached Bluetooth names and signal strength, with live
name filtering and optional unnamed results. Adding selects one specific peripheral;
connecting remains an explicit action. **Find by name** remains available for a
device that has not appeared in the browser yet.

The production app uses the daemon's origin for all API requests. Trainer controls
use the same WebSocket session that requested ownership. Closing that connection
releases ownership; reconnecting never resends control commands. Name selections
from the dashboard last until the daemon stops.

## Develop

Install Node.js 24 and Bun, then run from this directory:

```sh
bun install --frozen-lockfile
bun run dev
```

In a second terminal, run the daemon from the repository root:

```sh
cargo run -p bikebridge-cli -- run --mock
```

Open the Vite URL printed in the terminal. The dev server proxies `/api` and `/ws`
to `127.0.0.1:9376`. Use `run` without `--mock` for Bluetooth hardware. Test load
controls with mock mode before using physical equipment.

## Check and rebuild

```sh
bun run check
bun run build
```

Commit `dist/` along with source changes. Rust embeds these files at compile time,
so rebuild/restart the daemon after updating them. End users only need the Rust
executable, without a JavaScript runtime or separate web server.
