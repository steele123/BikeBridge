# Shimano Di2 and SRAM AXS inputs

Di2 and AXS can send mapped button actions through the existing BikeControl BLE
bridge. BikeBridge's OpenBikeControl driver already handles these actions; no new
daemon flag or controller-specific driver is needed.

```text
Di2 D-Fly / AXS buttons → BikeControl → OpenBikeControl BLE → BikeBridge → your app
```

Run BikeControl on a separate phone or computer that can advertise a BLE peripheral.
BikeBridge receives one aggregate bridge device. It does not directly pair with
Di2/AXS, control physical derailleurs, or expose drivetrain battery/gear telemetry.
Physical compatibility has not been tested here; available buttons and gestures
depend on the hardware, firmware, and BikeControl version/features.

## Shimano Di2

1. In Shimano E-Tube, assign the buttons you want to use to **D-Fly channels**.
   Your setup must expose D-Fly over Bluetooth; the Di2 name alone does not establish
   compatibility for every generation or component combination.
2. Connect the Di2 system in BikeControl, following its pairing instructions.
   Press the configured buttons so its D-Fly channel entries appear.
3. Map the channels to OpenBikeControl actions in BikeControl. The table below is
   an example layout you choose, not an automatic BikeBridge configuration.

| D-Fly channel | Suggested action | BikeBridge input |
| --- | --- | --- |
| 1 | Shift down | `shift_down` |
| 2 | Shift up | `shift_up` |
| 3 | Select | `confirm` |
| 4 | Back | `back` |

Use whichever channels your setup exposes. Short, double, and long presses are
interpreted by BikeControl and forwarded as mapped actions. BikeBridge does not
infer gestures from the timing or generate extra shift repeats while a button is
held. Restore your preferred button assignments in E-Tube when returning to normal
drivetrain use.

## SRAM AXS

Use **BikeControl 6.3 or newer** for individually mapped AXS buttons. Wake the
drivetrain, connect to the derailleur in BikeControl, and follow **Set up SRAM
control**, including its physical AXS-button authorization prompt when shown.
BikeControl saves the existing button configuration and changes the buttons to
report inputs. Use its **Restore original shifting** flow to restore that backup
before returning to outdoor shifting; disconnecting BikeBridge is not that restore
operation. See [BikeControl's AXS setup guide](https://bikecontrol.app/sram-axs/).

| AXS button | Suggested action | BikeBridge input |
| --- | --- | --- |
| Left paddle | Shift down | `shift_down` |
| Right paddle | Shift up | `shift_up` |
| Available auxiliary button | Select | `confirm` |
| Another available auxiliary button | Back | `back` |

Assign only the controls present on your bike and recognized by BikeControl.
Simultaneous left/right actions remain separate events; BikeBridge does not turn
them into a front-derailleur command. Map gestures in BikeControl if needed.

## Connect and record

In BikeControl choose **OpenBikeControl Compatible** and enable **Connect using
Bluetooth**. On the BikeBridge computer:

```sh
bikebridge run --record my-shifters.biketrace
# In a second terminal:
bikebridge devices
node examples/javascript/controller.mjs ble-<bridge-id>
```

Choose the **BikeControl bridge ID**, not the physical derailleur. Once BikeControl
shows app ID `bikebridge`, verify press/release events in the viewer. To disconnect,
run `bikebridge disconnect <bridge-id>`; closing the viewer leaves the bridge
connected. All received inputs are included in the recording.

Input actions do not automatically change trainer resistance. Your app can map
them to trainer commands using the normal ownership and safety rules. Recordings
preserve mapped actions, not the original D-Fly channel, AXS button identity, or
gesture name. Keep the chosen mappings with any bug report.

See [bridge connection behavior](zwift-controllers.md#connection-behavior) for
held-button cleanup, retries, and the distinction between losing a physical shifter
and losing the BLE bridge.

## Try without hardware

These **synthetic API traces** demonstrate the suggested layouts. They are
hand-authored scenarios, not captures from Di2 or AXS hardware or evidence of
physical compatibility. Their devices use `transport: "mock"`.

```sh
bikebridge replay examples/traces/di2-dfly-demo.biketrace
# In another terminal, subscribe and start playback:
node examples/javascript/replay.mjs
```

Stop that daemon, then repeat with `examples/traces/axs-paddles-demo.biketrace`.
Playback starts paused so the viewer sees the complete sequence. Both traces work
with `bikebridge trace-info <file>` and the existing replay controls.

- **Di2 layout, 5 seconds:** down/up taps, select/back taps, two rapid up taps,
  then a held down action released before a simulated bridge disconnect.
- **AXS layout, 5 seconds:** left/right taps, simultaneous down/up actions,
  a select tap, then a held up action released before a simulated bridge loss,
  reconnect, and a fresh up tap. Reconnect does not replay the held press.

For physical validation, record the drivetrain and lever models, firmware,
BikeControl version/platform, host OS, and mappings. Check taps, holds, configured
gestures, simultaneous buttons, a quiet coasting interval, bridge loss while held,
and reconnect. Verify AXS restoration separately in BikeControl.

## Protocol references

- [BikeControl supported devices and Di2 prerequisites](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/README.md).
- [BikeControl Di2 channel and gesture handling](https://github.com/OpenBikeControl/bikecontrol/blob/579cd3b315c512343d498a2faf7970b91a8a9b0a/lib/bluetooth/devices/shimano/shimano_di2.dart).
- [BikeControl AXS setup and restoration](https://bikecontrol.app/sram-axs/).
- [OpenBikeControl bridge protocol and verification](zwift-controllers.md#verification-and-sources).
