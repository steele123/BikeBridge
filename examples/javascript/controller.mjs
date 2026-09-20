// Node.js 22+. Pair physical controllers in BikeControl, enable its OpenBikeControl BLE bridge,
// then copy the bridge's ble-... ID from `bikebridge devices`.
const deviceId = process.argv[2];
if (!deviceId) {
  console.error("Usage: node examples/javascript/controller.mjs <device-id> [ws://127.0.0.1:9376/ws]");
  process.exit(1);
}
const socket = new WebSocket(process.argv[3] ?? "ws://127.0.0.1:9376/ws");
let ready = false;
let timer = setTimeout(() => { console.error("Controller setup timed out."); process.exitCode = 1; socket.close(); }, 30_000);
socket.addEventListener("open", () => socket.send(JSON.stringify({
  type: "subscribe", requestId: "inputs", events: ["input", "device", "error"],
})));
socket.addEventListener("message", ({ data }) => {
  const event = JSON.parse(data);
  if (event.type === "response") {
    if (!event.success) {
      console.error(event.error);
      process.exitCode = 1;
      socket.close();
    } else if (event.requestId === "inputs") {
      socket.send(JSON.stringify({ type: "device.connect", requestId: "connect", deviceId }));
    } else if (event.requestId === "connect") {
      clearTimeout(timer);
      ready = true;
      console.log("Listening:", event.data);
      console.log("Closing this viewer leaves the controller available. Use bikebridge disconnect <id> to release it.");
    }
  } else if (event.type === "input" && event.deviceId === deviceId) {
    console.log(JSON.stringify(event));
  } else if (event.type === "error" || event.deviceId === deviceId || event.data?.id === deviceId) {
    console.log(JSON.stringify(event));
  }
});
socket.addEventListener("error", () => { console.error("Cannot communicate with BikeBridge."); process.exitCode = 1; socket.close(); });
socket.addEventListener("close", () => { clearTimeout(timer); if (!ready) process.exitCode = 1; });
process.on("SIGINT", () => socket.close());
process.on("SIGTERM", () => socket.close());
