# Shared product code

`heart-rate.ts` resolves a selected monitor by ID or a unique remembered name,
and enforces independent five-second freshness. An unavailable selected monitor
returns no BPM; it never substitutes the bike's reading.

Ride Along and Stream Overlay import this framework-independent module. Both
products' `bun run test` commands include `heart-rate.test.ts`.
