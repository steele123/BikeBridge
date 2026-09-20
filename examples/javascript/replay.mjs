// Node.js 22+. Start `bikebridge replay examples/traces/demo-ride.biketrace` first.
const base = new URL(process.argv[2] ?? "http://127.0.0.1:9376");
const url = new URL("/ws", base);
url.protocol = base.protocol === "https:" ? "wss:" : "ws:";
const socket = new WebSocket(url);
let timer;
let closing = false;
let checking = false;
function close() {
  closing = true;
  clearInterval(timer);
  socket.close();
}
socket.addEventListener("open", () => socket.send(JSON.stringify({
  type: "subscribe", requestId: "all",
  events: ["telemetry", "input", "device", "scan", "error", "command", "session", "replay"],
})));
socket.addEventListener("message", async ({ data }) => {
  try {
    const message = JSON.parse(data);
    console.log(JSON.stringify(message));
    if (message.type === "response" && message.requestId === "all") {
      if (!message.success) throw new Error("Subscription failed.");
      const response = await fetch(new URL("/api/replay/start", base), { method: "POST", signal: AbortSignal.timeout(5000) });
      if (!response.ok) throw new Error(`Replay start failed: ${await response.text()}`);
      timer = setInterval(async () => {
        if (checking || closing) return;
        checking = true;
        try {
          const response = await fetch(new URL("/api/replay", base), { signal: AbortSignal.timeout(5000) });
          if (!response.ok) throw new Error("Replay status failed.");
          const status = await response.json();
          if (status?.finished) {
            // The server has emitted all events. Leave a short drain window for queued socket frames.
            clearInterval(timer);
            setTimeout(close, 250);
          }
        } catch (error) {
          console.error(error.message);
          process.exitCode = 1;
          close();
        } finally { checking = false; }
      }, 250);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
    close();
  }
});
socket.addEventListener("error", () => {
  console.error("Cannot communicate with BikeBridge replay.");
  process.exitCode = 1;
  close();
});
socket.addEventListener("close", () => {
  if (!closing) process.exitCode = 1;
  clearInterval(timer);
});
process.on("SIGINT", close);
process.on("SIGTERM", close);
