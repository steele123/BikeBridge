# FTMS trainer control

Phase 4 adds exclusive WebSocket control for connected FTMS indoor-bike trainers.
The implementation is tested with injected GATT transports; no physical model or
firmware has passed acceptance testing in this environment.

## Discovery and supported modes

Control requires Fitness Machine Control Point `0x2AD9` with WRITE and INDICATE,
plus Fitness Machine Status `0x2ADA` with NOTIFY, under service `0x1826`.
Notification receivers are installed before subscribing. The second uint32 in
Fitness Machine Feature gates resistance (bit 2), power (bit 3), and indoor-bike
simulation (bit 13). Start, Stop, Reset, and Request Control use the Control Point.

Resistance and power additionally require their readable supported-range
characteristics (`0x2AD6`, `0x2AD8`). BikeBridge accepts exactly six bytes:
signed int16 minimum, signed int16 maximum, unsigned int16 increment, little endian.
It rejects reversed or degenerate ranges, zero increments, increments wider than
the range, and negative resistance minima. Missing or invalid ranges disable only
that mode. A power range with no supported nonnegative value below the configured
ceiling does not advertise ERG control.

Normalized resistance maps linearly between the device's raw minimum and maximum,
then snaps down to the increment grid anchored at the minimum. Zero means the
device minimum, which is not necessarily zero physical force. ERG targets intersect
the device range and configured ceiling, then snap down to supported watts.
Responses expose the resulting value. Simulation values truncate toward zero to
their wire resolution after clamping.

## Wire format and sources

All multibyte fields are little endian. These are Control Point writes:

| API operation | Opcode and parameters |
| --- | --- |
| Request Control | `00` |
| Reset | `01` |
| Set Resistance | `04`, signed int16 raw level (0.1 unit resolution) |
| Set Target Power | `05`, signed int16 watts |
| Start/Resume | `07` |
| Stop | `08 01`, followed by Reset |
| Indoor Bike Simulation | `11`, wind int16 (0.001 m/s), grade int16 (0.01%), rolling coefficient uint8 (0.0001), wind coefficient uint8 (0.01 kg/m) |

References used to select and verify the layout:

- [Bluetooth SIG FTMS 1.0.1](https://www.bluetooth.com/specifications/specs/fitness-machine-service-1-0-1/), sections 4.3 and 4.16–4.19, for features and procedures.
- [Bluetooth SIG Errata Service Release 11](https://www.bluetooth.org/DocMan/handlers/DownloadDoc.ashx?doc_id=436247), E9135 and E8991, PDF pages 183–184, correcting the resistance control parameter to signed 16-bit with 0.1 resolution.
- [Bluetooth SIG FTMS test suite, revision p6](https://files.bluetooth.com/wp-content/uploads/dlm_uploads/2024/10/FTMS.TS_.p6.pdf), table 4.4 for six-byte resistance/power ranges, and control-write tests for signed 16-bit target fields.

Published SIG materials disagree about resistance widths: the online service
table and current GSS include uint8 definitions. The control path explicitly uses
the errata/test-suite profile above. It does not guess between layouts. Three-byte
resistance ranges disable resistance control. The existing [Indoor Bike Data
parser](ftms.md) still consumes a uint8 resistance measurement; two-byte resistance
measurement packets remain unsupported. Control encoding and telemetry parsing
must be validated separately on each physical trainer.

## Ownership and acknowledgements

The first supported command acquires local ownership and requests FTMS control.
Explicit `trainer.requestControl` is available. A second client receives
`trainer_control_denied`; HTTP disconnect cannot bypass the owner's lease.
Reset releases both FTMS permission and local ownership. Stop sends Stop then
Reset, retains local ownership, and reacquires FTMS permission on the next command.

Each transaction waits for both the GATT write result and a three-byte indication
`80 <requested opcode> <result>`, within three seconds total. FTMS results map to
success, unsupported operation, invalid value, control failure, or control denied.
Malformed, mismatched, duplicate, or unexpected indications are failures. Telemetry,
WebSocket subscriptions, and heartbeat handling remain active during the wait.
Only one device operation may be pending on a WebSocket at once.

Default ceilings are 800 W, absolute grade 15%, and normalized resistance 0.7.
Resistance smoothing defaults to 0.2 normalized units per second. Entering
resistance mode establishes the device minimum with an acknowledged write. An
accepted response then reports the quantized destination; it does not mean the
ramp has completed. A 250 ms ticker raises resistance at the configured rate,
measuring elapsed time since the previous confirmed step. Every step needs an
acknowledgement. Decreases bypass the slew limit at the next tick. Stop, Reset,
mode changes, and user-stop status cancel pending ramps. ERG and simulation changes
are immediate commands, without smoothing.

## Cleanup and reconnection

Owner exit, daemon shutdown, explicit disconnect, and control failures attempt
Stop then Reset before OS disconnect when permission is still held and the
transaction channel is synchronized. A lost acknowledgement, failed write, or
cancellation leaves an uncertain outcome: BikeBridge disconnects without starting
another potentially overlapping procedure. Revoked control is not reacquired
during cleanup. A lost link may prevent any safe-state command from reaching the
trainer. Successful protocol acknowledgement is not physical load verification.

`[trainer].auto_reconnect = true` opts into at most five retries after link loss,
with delays of 1, 2, 5, 10, and 30 seconds. It defaults to false. Each scheduled retry
emits `device.reconnecting` with `deviceId`, `attempt`, and `delaySeconds`.
Successful reconnect restores telemetry only: ownership and load targets are never
replayed. Explicit disconnect cancels retries. Owner exit and control-command
failures do not start automatic retries.

## Hardware acceptance

1. Wake the trainer and run `bikebridge run`. Use `bikebridge devices` to find its
   session ID. Check telemetry with `node examples/javascript/trainer.mjs <id>`.
2. Run `node examples/javascript/control.mjs <id>`. Check its printed capabilities.
   Enter `start` if the trainer requires a started workout, then request a low
   resistance such as `resistance 0.1`. Compare the physical change with the target.
3. Exercise supported ERG and grade modes at modest targets, then Stop and Reset.
   Confirm resistance ramps, ceilings, and control-denied behavior with two clients.
4. Close the owner, interrupt the daemon, and interrupt the BLE link in separate
   trials. Record actual trainer behavior; do not assume disconnect removes load.
5. With automatic reconnect enabled, verify reconnect events and that a recovered
   session receives no load command until the client explicitly requests it.

Record model, firmware, OS, verified capabilities, range bytes, observed physical
behavior, and typed errors. Windows native builds and simulated transports passing
do not establish Linux/macOS execution or hardware compatibility.
