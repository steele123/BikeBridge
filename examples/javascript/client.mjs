// Node.js 22+ built-in WebSocket; no packages or Bluetooth knowledge required.
const socket = new WebSocket(process.argv[2] ?? "ws://127.0.0.1:9376/ws");
socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ type: "subscribe", requestId: "sub", events: ["telemetry", "input"] }));
  socket.send(JSON.stringify({ type: "trainer.setTargetPower", requestId: "erg", deviceId: "mock-trainer", data: { watts: 250 } }));
  socket.send(JSON.stringify({ type: "mock.input", requestId: "shift", deviceId: "mock-controller", data: { input: "shift_up", state: "pressed" } }));
});
socket.addEventListener("message", ({ data }) => {
  const event = JSON.parse(data);
  if (event.type === "telemetry") console.log(`${event.deviceId}: ${event.data.powerWatts} W, ${event.data.cadenceRpm.toFixed(1)} RPM`);
  else console.log(event);
  if (event.type === "response" && !event.success) process.exitCode = 1;
});
socket.addEventListener("error", (event) => { console.error("WebSocket failed:", event.message); process.exitCode = 1; });
socket.addEventListener("close", () => console.log("Disconnected; trainer control released."));
process.on("SIGINT", () => socket.close());
