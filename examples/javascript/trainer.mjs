// Node.js 22+. Start `bikebridge run`, then copy an ID from `bikebridge devices`.
// Usage: node examples/javascript/trainer.mjs <device-id> [http://127.0.0.1:9376]
const deviceId = process.argv[2];
if (!deviceId) {
  console.error("Usage: node examples/javascript/trainer.mjs <device-id> [http://127.0.0.1:9376]");
  console.error("Run `bikebridge devices` to find your trainer's ID.");
  process.exit(1);
}
const base = new URL(process.argv[3] ?? "http://127.0.0.1:9376");
const wsUrl = new URL("/ws", base);
wsUrl.protocol = base.protocol === "https:" ? "wss:" : "ws:";
const socket = new WebSocket(wsUrl);
let deadline = setTimeout(() => fail("Timed out connecting to BikeBridge."), 5000);

function fail(message) {
  console.error(message);
  process.exitCode = 1;
  clearTimeout(deadline);
  socket.close();
}
socket.addEventListener("open", () => {
  clearTimeout(deadline);
  deadline = setTimeout(() => fail("Trainer connection timed out."), 20000);
  // Error events have no deviceId; leave them unfiltered and filter telemetry locally.
  socket.send(JSON.stringify({type: "subscribe", requestId: "subscribe", events: ["telemetry", "device", "error"]}));
});
socket.addEventListener("message", ({data}) => {
  const event = JSON.parse(data);
  if (event.type === "response") {
    if (!event.success) return fail(`${event.error.code}: ${event.error.message}`);
    if (event.requestId === "subscribe") {
      socket.send(JSON.stringify({type: "device.connect", requestId: "connect", deviceId}));
    } else if (event.requestId === "connect") {
      clearTimeout(deadline);
      console.log(`Connected to ${event.data.name}. Pedal to receive measurements. Ctrl+C closes this viewer.`);
    }
  } else if (event.type === "telemetry" && event.deviceId === deviceId) {
    const {powerWatts, cadenceRpm, speedKph} = event.data;
    console.log(`${powerWatts ?? "—"} W | ${cadenceRpm ?? "—"} rpm | ${speedKph ?? "—"} km/h`);
  } else if (event.type === "device.disconnected" && event.data.id === deviceId) {
    fail("Trainer disconnected. Run this example again to reconnect.");
  } else if (event.type === "error") {
    console.error(`${event.data.code}: ${event.data.message}`);
  }
});
socket.addEventListener("error", () => fail("BikeBridge WebSocket failed."));
socket.addEventListener("close", () => {
  clearTimeout(deadline);
  console.log(`Viewer closed. To end the shared BLE session: bikebridge disconnect ${deviceId}`);
});
process.on("SIGINT", () => socket.close());
