export type Device = {
  id: string; name: string; kind: string; transport: string; connected: boolean;
  signalStrength?: number; capabilities: string[];
};
export type Telemetry = {
  powerWatts?: number; cadenceRpm?: number; speedKph?: number; heartRateBpm?: number;
  distanceMeters?: number; timestampMs: number;
};
export type Status = {
  version: string; uptimeSeconds: number; bluetoothAvailable: boolean; bluetoothEnabled: boolean;
  mockMode: boolean; connectedDevices: number;
  scan: { scanning: boolean; lastError?: { message: string } };
  replay?: { playing: boolean; finished: boolean; speed: number; positionUs: number; durationUs: number };
};
// The console intentionally exposes arbitrary versioned API envelopes.
export type Envelope = { type: string; deviceId?: string; requestId?: string; success?: boolean; data?: any; error?: { code: string; message: string }; [key: string]: any };
export type ConnectionState = 'connecting' | 'online' | 'offline';

export async function http(path: string, method = 'GET', body?: unknown) {
  const response = await fetch(path, {
    method, signal: AbortSignal.timeout(15000),
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let value: any;
  try { value = JSON.parse(text); } catch { throw new Error(text || `HTTP ${response.status}`); }
  if (!response.ok) throw new Error(value.data?.message ?? value.error?.message ?? `HTTP ${response.status}`);
  return value;
}

export class BridgeClient {
  private socket?: WebSocket;
  private stopped = false;
  private reconnect?: ReturnType<typeof setTimeout>;
  private attempt = 0;
  private sequence = 0;
  private pending = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  constructor(private message: (value: Envelope) => void, private state: (value: ConnectionState) => void) {}
  start() {
    if (this.stopped) return;
    this.state('connecting');
    const socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`);
    this.socket = socket;
    socket.onmessage = ({ data }) => {
      let event: Envelope;
      try { event = JSON.parse(data); } catch { return; }
      if (event.type === 'hello') {
        this.attempt = 0;
        this.state('online');
        void this.send({ type: 'subscribe', events: ['telemetry', 'device', 'input', 'scan', 'error', 'command', 'replay', 'session'] })
          .catch(error => this.message({ type: 'error', data: { message: error.message } }));
      }
      if (event.type === 'response' && event.requestId) {
        const task = this.pending.get(event.requestId);
        if (task) {
          clearTimeout(task.timer); this.pending.delete(event.requestId);
          if (event.success) task.resolve(event.data);
          else task.reject(new Error(event.error?.message ?? 'Command failed.'));
        }
      }
      this.message(event);
    };
    socket.onclose = () => {
      this.failPending('Connection lost. The command outcome may be unknown; refresh device state before retrying.');
      if (!this.stopped) {
        this.state('offline');
        this.reconnect = setTimeout(() => this.start(), Math.min(1000 * 2 ** this.attempt++, 10000));
      }
    };
    socket.onerror = () => socket.close();
  }
  send(command: Record<string, unknown>): Promise<any> {
    const socket = this.socket;
    if (socket?.readyState !== WebSocket.OPEN) return Promise.reject(new Error('BikeBridge is not connected.'));
    const requestId = `web-${++this.sequence}`;
    const envelope = { ...command, requestId };
    this.message({ ...envelope, type: String(command.type), direction: 'out' });
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(requestId);
        reject(new Error('Command timed out. Check device state before retrying.'));
      }, 30000);
      this.pending.set(requestId, { resolve, reject, timer });
      socket.send(JSON.stringify(envelope));
    });
  }
  private failPending(reason: string) {
    for (const task of this.pending.values()) { clearTimeout(task.timer); task.reject(new Error(reason)); }
    this.pending.clear();
  }
  stop() {
    this.stopped = true; clearTimeout(this.reconnect);
    this.failPending('Viewer closed.'); this.socket?.close();
  }
}
