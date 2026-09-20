// Node.js 22+. Every load change is entered explicitly by the operator.
import { createInterface } from "node:readline";

const deviceId = process.argv[2];
if (!deviceId) {
  console.error("Usage: node examples/javascript/control.mjs <device-id> [ws://127.0.0.1:9376/ws]");
  process.exit(1);
}
const socket = new WebSocket(process.argv[3] ?? "ws://127.0.0.1:9376/ws");
const pending = new Map();
let sequence = 0;
let lines;
let closing = false;

function close() {
  closing = true;
  lines?.close();
  socket.close(); // The daemon releases this client's lease and attempts cleanup.
}

socket.addEventListener("message", ({ data }) => {
  const message = JSON.parse(data);
  if (message.type === "response") {
    const waiter = pending.get(message.requestId);
    if (!waiter) return;
    pending.delete(message.requestId);
    clearTimeout(waiter.timer);
    if (message.success) waiter.resolve(message.data);
    else waiter.reject(new Error(`${message.error.code}: ${message.error.message}`));
  } else if (message.type !== "hello") {
    console.log(JSON.stringify(message));
  }
});
socket.addEventListener("close", () => {
  if (!closing) process.exitCode = 1;
  for (const waiter of pending.values()) {
    clearTimeout(waiter.timer);
    waiter.reject(new Error("WebSocket closed; command outcome may be uncertain."));
  }
  pending.clear();
  lines?.close();
  console.log("Disconnected; trainer cleanup requested by session closure.");
});

function request(type, data) {
  if (socket.readyState !== WebSocket.OPEN) return Promise.reject(new Error("WebSocket is not open."));
  const requestId = String(++sequence);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(requestId);
      reject(new Error("Response timed out; closing without retrying an uncertain command."));
      close();
    }, 30_000);
    pending.set(requestId, { resolve, reject, timer });
    const command = { type, requestId };
    if (type === "subscribe") Object.assign(command, data);
    else {
      command.deviceId = deviceId;
      if (data !== undefined) command.data = data;
    }
    socket.send(JSON.stringify(command));
  });
}

function parse(line) {
  const [verb, value, ...extra] = line.trim().split(/\s+/);
  if (["start", "stop", "reset", "control", "connect"].includes(verb)) {
    if (value !== undefined) throw new Error("This command takes no value.");
    return [verb === "connect" ? "device.connect" : `trainer.${verb === "control" ? "requestControl" : verb}`];
  }
  const number = Number(value);
  if (extra.length || value === undefined || !Number.isFinite(number)) {
    throw new Error("Use start, resistance <0..1>, erg <watts>, grade <percent>, stop, reset, control, connect, or quit.");
  }
  switch (verb) {
    case "resistance": return ["trainer.setResistance", { resistance: number }];
    case "erg":
      if (!Number.isInteger(number) || number < 0 || number > 65535) throw new Error("Watts must be an integer from 0 to 65535.");
      return ["trainer.setTargetPower", { watts: number }];
    case "grade": return ["trainer.setSimulation", { gradePercent: number, windSpeedMps: 0, crr: 0.004, cw: 0.51 }];
    default: throw new Error("Unknown command.");
  }
}

process.on("SIGINT", close);
process.on("SIGTERM", close);
try {
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", () => reject(new Error("Cannot connect to BikeBridge.")), { once: true });
    socket.addEventListener("close", () => reject(new Error("Connection closed before startup.")), { once: true });
  });
  await request("subscribe", { events: ["device", "error"], deviceIds: [] });
  console.log("Connected device:", await request("device.connect"));
  await request("trainer.requestControl");
  console.log("Commands: start | resistance 0.1 | erg 150 | grade 2 | stop | reset | control | connect | quit");
  console.log("Resistance replies report a destination; the ramp continues asynchronously. No load is set automatically.");
  lines = createInterface({ input: process.stdin, output: process.stdout, terminal: Boolean(process.stdin.isTTY) });
  lines.on("SIGINT", close);
  for await (const line of lines) {
    if (!line.trim()) continue;
    if (line.trim() === "quit") break;
    try {
      console.log("Result:", await request(...parse(line)));
    } catch (error) {
      console.error(error.message);
    }
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
} finally {
  close();
}
