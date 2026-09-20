import type { Device, Sample } from './config';
export type FeedState = { online: boolean; devices: Device[]; samples: Record<string, Sample>; mode: 'live' | 'demo' | 'replay'; error: string; epoch: number };
export const emptyFeed = (): FeedState => ({ online: false, devices: [], samples: {}, mode: 'live', error: '', epoch: 0 });
// This product is a read-only API subscriber; it never claims trainer control.
export class Feed {
  private state = emptyFeed();
  private socket?: WebSocket;
  private stopped = false;
  private retry?: ReturnType<typeof setTimeout>;
  private poll?: ReturnType<typeof setInterval>;
  private delay = 1000;
  private revision = 0;
  private fetching = false;
  constructor(private update: (state: FeedState) => void) {}
  private emit() { this.update({ ...this.state, samples: { ...this.state.samples } }); }
  start() { this.open(); this.poll = setInterval(() => { if (this.state.online) void this.refresh(); }, 5000); }
  private async refresh() {
    if (this.fetching) return;
    this.fetching = true;
    const socket = this.socket, revision = this.revision;
    try {
      const values = await Promise.all(['/api/devices', '/api/status'].map(async path => {
        const response = await fetch(path, { signal: AbortSignal.timeout(10000) });
        if (!response.ok) throw new Error(`BikeBridge returned HTTP ${response.status}.`);
        return response.json();
      }));
      if (this.stopped || this.socket !== socket || !this.state.online) return;
      const [devices, status] = values;
      if (status.protocolVersion !== 1) throw new Error('Unsupported BikeBridge protocol version.');
      if (this.revision === revision) this.state.devices = devices;
      this.state.mode = status.mockMode ? 'demo' : status.replay ? 'replay' : 'live';
      this.emit();
    } catch (error) { if (!this.stopped && this.socket === socket) { this.state.error = String(error); this.emit(); } }
    finally { this.fetching = false; }
  }
  private open() {
    if (this.stopped) return;
    const socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`);
    this.socket = socket;
    socket.onmessage = ({ data }) => {
      if (this.stopped || socket !== this.socket) return;
      let event;
      try { event = JSON.parse(data); } catch { socket.close(); return; }
      if (event.type === 'hello') {
        if (event.protocolVersion !== 1) { this.state.error = 'Unsupported BikeBridge protocol version.'; socket.close(); return; }
        socket.send(JSON.stringify({ type: 'subscribe', requestId: 'overlay', events: ['telemetry', 'device', 'error', 'replay'] }));
      } else if (event.type === 'response' && event.requestId === 'overlay') {
        if (!event.success) { socket.close(); return; }
        this.delay = 1000; this.state.online = true; this.state.error = ''; void this.refresh();
      } else if (event.type === 'telemetry' && event.deviceId) {
        this.state.samples[event.deviceId] = { at: performance.now(), data: event.data };
      } else if (['device.discovered', 'device.updated', 'device.connected', 'device.disconnected'].includes(event.type)) {
        this.revision++;
        this.state.devices = [...this.state.devices.filter(device => device.id !== event.data.id), event.data];
        if (!event.data.connected) delete this.state.samples[event.data.id];
      } else if (event.type === 'replay.reset' || (event.type === 'error' && event.data?.code === 'events_lost')) {
        this.state.samples = {}; this.state.epoch++; void this.refresh();
      } else if (event.type === 'error') this.state.error = event.data?.message ?? 'BikeBridge reported an error.';
      this.emit();
    };
    socket.onerror = () => socket.close();
    socket.onclose = () => {
      if (this.stopped) return;
      this.state.online = false; this.state.samples = {}; this.state.devices = []; this.state.epoch++; this.emit();
      this.retry = setTimeout(() => this.open(), this.delay); this.delay = Math.min(this.delay * 2, 10000);
    };
  }
  stop() { this.stopped = true; clearTimeout(this.retry); clearInterval(this.poll); this.socket?.close(); }
}
