// Node.js 22+. Start BikeBridge with `bikebridge run` (without --mock).
const base = new URL(process.argv[2] ?? "http://127.0.0.1:9376");
const wsUrl = new URL("/ws", base);
wsUrl.protocol = "ws:";
const socket = new WebSocket(wsUrl);
socket.addEventListener("open", () => {
  socket.send(JSON.stringify({ type: "subscribe", requestId: "discovery", events: ["device", "scan", "error"] }));
});
socket.addEventListener("message", async ({ data }) => {
  const event = JSON.parse(data);
  console.log(event);
  if (event.type !== "response" || event.requestId !== "discovery") return;
  try {
    if (!event.success) throw new Error(event.error.message);
    const scan = await fetch(new URL("/api/scan/start", base), { method: "POST", signal: AbortSignal.timeout(20000) });
    const result = await scan.json();
    if (!scan.ok) throw new Error(result.data.message);
    const devices = await fetch(new URL("/api/devices", base), { signal: AbortSignal.timeout(5000) });
    if (!devices.ok) throw new Error(`Device snapshot failed (${devices.status})`);
    console.log("Known devices:", await devices.json());
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
    socket.close();
  }
});
socket.addEventListener("error", () => { console.error("Cannot connect to BikeBridge."); process.exitCode = 1; });
socket.addEventListener("close", () => console.log("Discovery client disconnected. Use `bikebridge scan --stop` to stop the shared scan."));
process.on("SIGINT", () => socket.close());
