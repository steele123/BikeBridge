# Zwift controller inputs through BikeControl

BikeBridge now receives controller inputs through the OpenBikeControl BLE bridge:

```text
Zwift Click / Play / Ride → BikeControl → OpenBikeControl BLE → BikeBridge → your app
```

[BikeControl](https://github.com/OpenBikeControl/bikecontrol) provides the physical
controller pairing and its firmware-specific behavior. BikeBridge discovers the
bridge, connects explicitly, identifies itself, and publishes normalized inputs.
Inputs and bridge disconnect/reconnect events are included in `.biketrace` recordings.
The same bridge accepts mapped [Shimano Di2 and SRAM AXS inputs](di2-axs.md);
that guide covers their pairing prerequisites and hardware-free demo traces.

This implements a bridge integration, not native pairing to proprietary Zwift
controller services, a Zwift game integration, or a Zwift-certified product.
No physical Click, Play, or Ride has been tested with this implementation yet.

## Setup

1. Install BikeControl on a phone or a second computer capable of BLE peripheral
   advertising. Use a separate device for the bridge; same-machine BLE loopback is
   not a supported setup here.
2. Pair your Click, Play, or Ride in BikeControl. Complete any onboarding or
   firmware-specific setup that BikeControl requests.
3. Select **OpenBikeControl Compatible** as the target app and enable its
   **Connect using Bluetooth** option. This advertises a bridge named BikeControl.
4. On the BikeBridge computer, run:

   ```sh
   bikebridge run --record controllers.biketrace
   # In another terminal:
   bikebridge devices
   node examples/javascript/controller.mjs ble-<bridge-id>
   ```

5. BikeControl should show the connected app ID `bikebridge`. Configure its button
   mappings for the actions you need, such as shifting, steering, confirm, or back.
   Press and release buttons and check the viewer's `input` events.

The bridge appears as `kind: "bike_controller"`, `transport: "bluetooth"`, with
the verified `controller_input` capability after connection. This means the input
channel is usable; it does not claim every physical controller has every button.
Several controllers paired in BikeControl share one bridge identity. The BLE
protocol does not attach a physical-controller ID to each action, so BikeBridge
cannot split that aggregate stream back into Click/Play/Ride identities.

To disconnect, use `bikebridge disconnect <bridge-id>`. Closing a viewer does not
disconnect the bridge. `run --mock` and replay mode do not initialize Bluetooth;
the viewer can use `mock-controller` to inspect injected inputs during development.

## Inputs

| OpenBikeControl action | BikeBridge input |
| --- | --- |
| Shift up / down | `shift_up` / `shift_down`, pressed/released |
| Flat gear selection | `gear`, value 1–100 (higher source positions clamp to 100) |
| Select / back | `confirm` / `back`, pressed/released |
| Steer left / right | `steering_left` / `steering_right`, pressed/released |
| Brake | `brake`, value 0–2: 1 is one full brake, 2 represents combined braking |
| Other advertised navigation, social, workout, camera, and power-up actions | `button`, with the OpenBikeControl action ID in `button` |

Generic digital actions use pressed/released. Generic analog actions use
`state: "value"` and preserve the raw source byte in `value` (2–255); applications
can interpret it using the public action definition. Chainring/cassette selection
and custom/vendor action IDs are not advertised in this implementation.

```json
{"type":"input","deviceId":"ble-<bridge-id>","data":{"input":"shift_up","state":"pressed"},"timestampMs":1789776000000}
{"type":"input","deviceId":"ble-<bridge-id>","data":{"input":"brake","state":"value","value":1.0},"timestampMs":1789776000100}
```

Button packets describe changes, not complete snapshots. Duplicate states are
suppressed; rapid press/release transitions remain in order. Input events do not
automatically change trainer resistance or acquire trainer control. An application
may map them to trainer commands through the existing ownership and safety rules.

## Connection behavior

The driver verifies the OpenBikeControl service, Button State NOTIFY property,
and writable App Information characteristic. It subscribes before sending app
information, with a ten-second setup deadline. BikeControl's current implementation
requires this handshake and an explicit supported-action list to forward inputs.
The metadata is written in consecutive 20-byte chunks using BikeControl's verified
reassembler; this compatibility choice has not been established for other
OpenBikeControl implementations that require one unfragmented write.

BLE message boundaries are preserved. Network/mDNS transport is not implemented;
the published variable-length TCP format does not define an unambiguous length
delimiter, so BikeBridge does not guess that TCP reads correspond to messages.
There is no new network listener or remote trainer access.

Bridge link loss, explicit disconnect, malformed packets, and daemon shutdown
release held digital inputs and emit a zero brake value before `device.disconnected`.
These cleanup inputs are synthesized by BikeBridge and recorded like other inputs.
Gear selections do not receive a synthetic release. Malformed packets are rejected
atomically and close the session. More than 1000 notifications in one second also
closes it. Unexpected loss retries up to five times after 1/2/5/10/30-second delays;
explicit disconnect cancels retries. Reconnection never replays held buttons.

These controller retries are independent of `[trainer].auto_reconnect`. Silence
is normal while no buttons are changing; the OS link is checked once per second.
Device connection state describes the bridge link. A physical controller leaving
BikeControl while its BLE bridge remains connected is not necessarily a separate
BikeBridge disconnect event.

## Verification and sources

Tests cover decoding, partial updates, rapid shifts, malformed data, brakes, gear
mapping, connection cancellation, held-input cleanup, bounded retries, and the path
from injected controller bytes to WebSocket events, recording, and replay. Windows
native builds compile the actual BLE transport. Hardware pairing and firmware
compatibility remain pending; Linux/macOS execution is not locally verified.

The protocol was independently implemented from these primary sources:

- [OpenBikeControl specification](https://github.com/OpenBikeControl/openbikecontrol-protocol/blob/c057e1d7ad05ceb1a6d59a0ca95e57394b7f9b48/PROTOCOL.md) and [BLE transport](https://github.com/OpenBikeControl/openbikecontrol-protocol/blob/c057e1d7ad05ceb1a6d59a0ca95e57394b7f9b48/BLE.md).
- [BikeControl BLE emulator](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/lib/bluetooth/devices/openbikecontrol/obc_ble_emulator.dart) and [app-information reassembler](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/lib/bluetooth/devices/openbikecontrol/app_info_reassembler.dart).

To establish compatibility, record the controller model, firmware, BikeControl
version/platform, and BikeBridge host OS. Check both press and release for mapped
actions, simultaneous buttons, bridge loss while a button is held, explicit
disconnect, and reconnect. Hardware-specific troubleshooting belongs first in
BikeControl's pairing/onboarding flow.
