export const metrics = ['power', 'cadence', 'speed', 'heart'] as const;
export type Metric = typeof metrics[number];
export type Config = { source: string; device: string; metrics: Metric[]; layout: 'bar' | 'stack'; theme: 'dark' | 'light'; accent: 'lime' | 'cyan' | 'orange' | 'pink'; unit: 'kph' | 'mph'; graph: boolean; label: boolean };
export const accents = { lime: '#c8f786', cyan: '#73dcff', orange: '#ffc185', pink: '#efa3dd' };
export function readConfig(search: string): Config {
  const query = new URLSearchParams(search);
  const selected = metrics.filter(metric => query.get('metrics')?.split(',').includes(metric));
  const accent = query.get('accent') ?? '';
  return {
    source: query.get('source') ?? '', device: query.get('device') ?? '', metrics: selected.length ? selected : ['power', 'cadence', 'speed'],
    layout: query.get('layout') === 'stack' ? 'stack' : 'bar', theme: query.get('theme') === 'light' ? 'light' : 'dark',
    accent: Object.hasOwn(accents, accent) ? accent as Config['accent'] : 'lime', unit: query.get('unit') === 'mph' ? 'mph' : 'kph',
    graph: query.get('graph') !== '0', label: query.get('label') !== '0'
  };
}
export function overlayUrl(origin: string, config: Config) {
  const query = new URLSearchParams({ view: 'overlay', ...config, metrics: config.metrics.join(','), graph: config.graph ? '1' : '0', label: config.label ? '1' : '0' });
  return `${origin}/overlay/?${query}`;
}
export type Device = { id: string; name: string; connected: boolean; kind: string; capabilities: string[] };
export function resolveSource(devices: Device[], config: Pick<Config, 'source' | 'device'>) {
  const exact = devices.find(device => device.id === config.device);
  if (exact && (!config.source || exact.name === config.source)) return exact;
  if (config.source) {
    const matches = devices.filter(device => device.name === config.source);
    return matches.length === 1 ? matches[0] : undefined;
  }
  if (config.device) return undefined;
  return devices.find(device => device.connected && device.capabilities.includes('power'));
}
export type Telemetry = { powerWatts?: number; cadenceRpm?: number; speedKph?: number; heartRateBpm?: number };
export type Sample = { at: number; data: Telemetry };
export function freshSample(sample: Sample | undefined, online: boolean, connected: boolean, now: number) {
  return online && connected && sample && now - sample.at < 5000 ? sample.data : undefined;
}

export type Point = { at: number; value?: number };
