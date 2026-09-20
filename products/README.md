# BikeBridge products

User-facing applications built on BikeBridge's HTTP/WebSocket API live here,
with one directory per product. The Rust daemon and hardware integrations remain
in `crates/`; small API usage examples remain in `examples/`.

## Current products

- [`dashboard/`](dashboard/README.md): device discovery, live telemetry, trainer
  controls, and the API console. Its compiled assets are embedded in the daemon.

- [`ride-along/`](ride-along/README.md): an always-on-top Svelte + Tauri desktop
  companion with live metrics, compact mode, and a local ride timer.

- [`stream-overlay/`](stream-overlay/README.md): a transparent Svelte overlay for
  OBS, with a live setup preview and configurable Browser Source URLs.

Keep each product's source, build configuration, and setup instructions together.
Products consume the public API. When multiple products need shared client code,
extract it into a shared package rather than importing one product's internals.
