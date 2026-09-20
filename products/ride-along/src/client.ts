import { Channel, invoke, isTauri } from '@tauri-apps/api/core';

export const native = isTauri();
export type Device = { id: string; name: string; kind: string; connected: boolean; capabilities: string[] };
export type Telemetry = { powerWatts?: number; cadenceRpm?: number; speedKph?: number; heartRateBpm?: number };
export type Connection = 'connecting' | 'online' | 'offline';
export type Envelope = { type: string; deviceId?: string; requestId?: string; success?: boolean; protocolVersion?: number; data?: any };
export type Snapshot = { devices: Device[]; status: { mockMode: boolean; replay?: unknown; protocolVersion: number } };

async function http(path: string, method = 'GET') {
  const response = await fetch(path, { method, signal: AbortSignal.timeout(15000) });
  const data = await response.json();
  if (!response.ok) throw new Error(data.data?.message ?? data.error?.message ?? `HTTP ${response.status}`);
  return data;
}
export async function snapshot(port: number): Promise<Snapshot> {
  if (native) return invoke('bridge_snapshot', { port });
  const [status, devices] = await Promise.all([http('/api/status'), http('/api/devices')]);
  return { status, devices };
}
export async function connectDevice(port: number, id: string) {
  return native ? invoke('bridge_connect_device', { port, id }) : http(`/api/devices/${encodeURIComponent(id)}/connect`, 'POST');
}
export class Stream {
  private stopped = false;
  private socket?: WebSocket;
  private channel?: Channel<Envelope>;
  private retry?: ReturnType<typeof setTimeout>;
  private delay = 1000;
  constructor(private port: number, private message: (event: Envelope) => void, private state: (state: Connection) => void) {}
  async start() {
    if (this.stopped) return;
    if (native) {
      this.channel = new Channel<Envelope>();
      this.channel.onmessage = event => {
        if (this.stopped) return;
        if (event.type === 'bridge.connection') this.state(event.data);
        else this.message(event);
      };
      await invoke('bridge_subscribe', { port: this.port, channel: this.channel });
    } else this.openBrowser();
  }
  private openBrowser() {
    if (this.stopped) return;
    this.state('connecting');
    const socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`);
    this.socket = socket;
    socket.onmessage = ({ data }) => {
      if (this.stopped) return;
      let event: Envelope;
      try { event = JSON.parse(data); } catch { socket.close(); return; }
      if (event.type === 'hello') {
        if (event.protocolVersion !== 1) { socket.close(); return; }
        socket.send(JSON.stringify({ type: 'subscribe', requestId: 'ride-along', events: ['telemetry', 'device', 'error', 'replay'] }));
      }
      if (event.type === 'response' && event.requestId === 'ride-along') {
        if (!event.success) { socket.close(); return; }
        this.delay = 1000; this.state('online');
      }
      this.message(event);
    };
    socket.onerror = () => socket.close();
    socket.onclose = () => {
      if (this.stopped) return;
      this.state('offline');
      this.retry = setTimeout(() => this.openBrowser(), this.delay);
      this.delay = Math.min(this.delay * 2, 10000);
    };
  }
  async stop() {
    this.stopped = true; clearTimeout(this.retry); this.socket?.close();
    if (native) await invoke('bridge_stop');
  }
}
