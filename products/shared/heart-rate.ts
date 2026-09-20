// Shared by the desktop companion and stream overlay. Each sensor has its own clock.
export type HeartSelection = { id: string; name: string };
export type HeartDevice = { id: string; name: string; kind: string; connected: boolean; capabilities: string[] };
export type HeartSample = { at: number; data: { heartRateBpm?: number } };
export function isHeartSource(device: HeartDevice) {
  return device.kind === 'heart_rate_monitor' || device.capabilities.includes('heart_rate');
}
export function resolveHeartSource(devices: HeartDevice[], selection: HeartSelection) {
  const exact = devices.find(device => device.id === selection.id && (!selection.name || device.name === selection.name));
  if (exact && isHeartSource(exact)) return exact;
  if (!selection.name) return undefined;
  const named = devices.filter(device => device.name === selection.name && isHeartSource(device));
  return named.length === 1 ? named[0] : undefined;
}
export function readHeartRate(devices: HeartDevice[], samples: Record<string, HeartSample>, selection: HeartSelection, now: number, online: boolean, bikeBpm?: number) {
  if (!online) return undefined;
  if (!selection.id && !selection.name) return bikeBpm;
  const device = resolveHeartSource(devices, selection);
  const sample = device && samples[device.id];
  if (!device?.connected || !sample || now - sample.at >= 5000 || now < sample.at) return undefined;
  const bpm = sample.data.heartRateBpm;
  return bpm !== undefined && Number.isFinite(bpm) ? bpm : undefined;
}
