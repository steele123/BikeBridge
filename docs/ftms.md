# FTMS telemetry

BikeBridge Phase 3 implements the read-only Indoor Bike Data path. These are the
sources used to verify the wire layout:

- [Bluetooth SIG Fitness Machine Service 1.0.1](https://www.bluetooth.com/specifications/specs/fitness-machine-service-1-0-1/), sections 4.3, 4.9, 4.18, and 4.19.
- [Bluetooth SIG GATT Specification Supplement](https://www.bluetooth.com/specifications/gss/), Indoor Bike Data; [machine-readable SIG definition](https://bitbucket.org/bluetooth-SIG/public/src/main/gss/org.bluetooth.characteristic.indoor_bike_data.yaml), checked 2026-09-19.
- [Bluetooth SIG Assigned Numbers](https://www.bluetooth.com/specifications/assigned-numbers/) for service and characteristic UUIDs.

## Service validation

Connection requires Fitness Machine Service `0x1826`, readable Fitness Machine
Feature `0x2ACC`, and notifying Indoor Bike Data `0x2AD2`. The eight-byte feature
value contains two little-endian 32-bit bitmaps. Speed is mandatory for indoor bikes;
measurement bits 1, 10, and 14 indicate cadence, heart rate, and power. Target-setting
bits do not grant any Phase 3 control capability.

## Notification layout

Flags are little-endian uint16. Fields follow in this order; all multi-byte values
are little-endian. Unpublished fields are consumed to preserve offsets.

| Flag | Field | Encoding | Public field |
| --- | --- | --- | --- |
| Bit 0 **clear** | Instantaneous speed | uint16 × 0.01 km/h | `speedKph` |
| 1 | Average speed | uint16 | omitted |
| 2 | Instantaneous cadence | uint16 × 0.5 RPM | `cadenceRpm` |
| 3 | Average cadence | uint16 | omitted |
| 4 | Total distance | uint24 meters | `distanceMeters` |
| 5 | Resistance level | uint8 | omitted |
| 6 | Instantaneous power | sint16 watts | `powerWatts` |
| 7 | Average power | sint16 watts | `averagePowerWatts` |
| 8 | Expended energy | uint16 + uint16 + uint8 | omitted |
| 9 | Heart rate | uint8 BPM | `heartRateBpm` |
| 10 | Metabolic equivalent | uint8 | omitted |
| 11 | Elapsed time | uint16 seconds | `elapsedTimeSeconds` |
| 12 | Remaining time | uint16 | omitted |

Bits 13–15 are reserved. BikeBridge rejects reserved bits, short packets, and
trailing bytes. Optional absent values stay absent; signed negative power is
preserved. Measurement timestamps are assigned by the daemon on receipt.

The current SIG GSS defines the resistance measurement as **uint8**. BikeBridge
uses that exact width, with no heuristic fallback to alternate encodings. Packets
using a two-byte resistance field are rejected by the strict length check. Such
hardware requires an explicitly verified transport-specific compatibility option
before it can be supported. Raw resistance is not the public normalized 0–1
resistance value; publishing it would require range verification and mapping.

## Complete records and failure policy

More Data (bit 0) is set on nonfinal fragments and cleared on the final fragment,
which includes speed. The assembler emits one sample on completion, then clears
all values. Disconnect/reconnect clears unfinished records. Malformed fragments
invalidate their record; the next final fragment establishes a new boundary.

BikeBridge additionally caps a record at 16 fragments and five seconds, and rejects
overlapping optional fields. These are application validation limits. Failed records
produce no telemetry and enter boundary resynchronization as needed. Error reporting
is limited to once every five seconds per device to keep corrupt streams from
flooding clients. A silent connected trainer produces no fabricated measurements.

## Verification status

Unit tests cover all 8,192 flag combinations, every truncation of a fully populated
packet, exact offsets, signed power, and fragment recovery. Session fixtures cover
timeouts, cleanup failures, cancellation, stream closure, and link loss. A real
loopback HTTP/WebSocket test connects an injected FTMS transport and verifies
250 W, 90.5 RPM, and 30 km/h decoded from notification bytes.

This is not physical hardware acceptance evidence. No trainer model or firmware
has been validated locally. Follow the [hardware check](devices.md) to establish
compatibility. Linux/macOS native execution also remains unverified here.
