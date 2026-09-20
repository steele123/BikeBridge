# Direct Zwift Click V2 input (experimental)

BikeBridge can discover the separate Click V2 left/right controllers and connect
directly to their plaintext Bluetooth input channel. No phone bridge is involved
in this path. Physical hardware and firmware compatibility remain unverified.

## Try it

1. Run `cargo run -p bikebridge-cli -- run` and open <http://127.0.0.1:9376>.
2. Wake both Click V2 controllers by pressing a button. Close other applications
   connected to them. They appear as **Zwift Click V2 (left)** and **(right)**.
3. Connect each side explicitly. Select a controller to open the dashboard's
   **Controller buttons** monitor, then press and release each button.
4. The API console shows `input` events; the JavaScript controller example also
   works with the controller's `ble-…` ID.

Some Click V2 firmware requires enabling both controllers in the **Zwift game**
before it will continue sending input to another application. Close Zwift after
that step, then reconnect in BikeBridge. This implementation sends the public
V2 handshake; it does not implement firmware unlocking, account/server exchanges,
or periodic reset workarounds. A successful handshake verifies the BLE channel,
not that a firmware unlock remains valid. A known stopped-input message becomes
an actionable error and releases held inputs; firmware that silently stops
sending data can still leave the transport connected. Test both sides before
relying on them. The [BikeControl bridge](zwift-controllers.md) remains available.

## Input mapping

| Physical button | API input | Generic button ID |
| --- | --- | --- |
| Left − | `shift_down` | — |
| Left arrow / right arrow | `steering_left` / `steering_right` | — |
| Up / down | `button` | `8193` / `8194` |
| Right + | `shift_up` | — |
| A / B | `confirm` / `back` | — |
| Y / Z | `button` | `8195` / `8196` |

Each event contains `state: "pressed"` or `"released"` and its controller's
device ID. Each connection decodes only its identified side's five buttons to
avoid duplicate inputs if firmware also forwards the other side's state. Frames
describe full button snapshots; repeated states are suppressed. The first frame
also publishes the initial released states. Disconnect and malformed data release
held buttons. Inputs do not automatically send trainer commands or change load.

## Identification and transport

- Zwift manufacturer company ID `0x094a`, model `0x0a` (right) or `0x0b` (left).
  Payloads need the model byte and the two advertised short-address bytes. The
  short address remains private. Other Zwift products are not matched.
- Service `0000fc82-0000-1000-8000-00805f9b34fb`, with legacy service
  `00000001-19ca-4651-86e5-fa29dcdd09d1` also accepted. A service/name alone does
  not establish a Click V2 model or side.
- Subscribe to characteristic `00000002-19ca-4651-86e5-fa29dcdd09d1` (notifications)
  and `00000004-19ca-4651-86e5-fa29dcdd09d1` (indications). Write `RideOn 02 03`
  to `00000003-19ca-4651-86e5-fa29dcdd09d1`. Await the V2 acknowledgement or a
  valid input frame before publishing a connected device.
- Opcode `0x23` carries a protobuf field 1 uint32 bitmap. Cleared bits mean pressed.
  Unknown protobuf fields are skipped with length/overflow checks. Unsupported
  wire types, missing/duplicate bitmap fields, and truncated frames fail atomically.
- Shared controller workers provide bounded setup, disconnect cleanup, five
  bounded link-loss retries, and the existing WebSocket/recording/replay path.

## Verification and references

Unit tests cover manufacturer identification, side filtering, known wire bytes,
all ten buttons, neutral/held states, malformed protobuf, and deduplication.
Integration tests inject raw V2 bytes through discovery, connection, WebSocket
delivery, and malformed-frame disconnect cleanup. These tests do not substitute
for checking your physical controller and firmware.

The Rust implementation is written independently from these public wire facts;
no third-party implementation or private unlocking code is bundled:

- [Makinolo's published keypad protocol and wire capture](https://www.makinolo.com/blog/2024/07/26/zwift-ride-protocol/).
- [BikeControl model IDs and service UUIDs](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/lib/bluetooth/devices/zwift/constants.dart).
- [Click V2 left handshake](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/lib/bluetooth/devices/zwift/zwift_clickv2_left_side.dart) and
  [right handshake](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/lib/bluetooth/devices/zwift/zwift_clickv2_right_side.dart).
- [Public firmware helper stub](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/prop_public/lib/devices/click_logic.dart): extra firmware setup is not published there.
