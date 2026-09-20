import { test, expect } from 'bun:test';
import { readHeartRate, resolveHeartSource, type HeartDevice } from './heart-rate';
const device: HeartDevice = { id: 'hr-1', name: 'Chest strap', kind: 'heart_rate_monitor', capabilities: ['heart_rate'], connected: true };
const selection = { id: device.id, name: device.name };
test('external heart rate has independent freshness and never falls back to trainer BPM', () => {
  const samples = { 'hr-1': { at: 1000, data: { heartRateBpm: 145 } } };
  expect(readHeartRate([device], samples, selection, 5999, true, 120)).toBe(145);
  expect(readHeartRate([device], samples, selection, 6000, true, 120)).toBeUndefined();
  expect(readHeartRate([{ ...device, connected: false }], samples, selection, 2000, true, 120)).toBeUndefined();
  expect(readHeartRate([device], samples, selection, 2000, false, 120)).toBeUndefined();
  expect(readHeartRate([device], samples, { id: '', name: '' }, 2000, true, 120)).toBe(120);
});
test('contact loss clears the previous rate; zero is a real value', () => {
  expect(readHeartRate([device], { 'hr-1': { at: 1000, data: {} } }, selection, 1001, true, 130)).toBeUndefined();
  expect(readHeartRate([device], { 'hr-1': { at: 1000, data: { heartRateBpm: 0 } } }, selection, 1001, true)).toBe(0);
});
test('remembered names survive new IDs but ambiguous or different sensors are not substituted', () => {
  const moved = { ...device, id: 'hr-new' };
  expect(resolveHeartSource([moved], selection)?.id).toBe('hr-new');
  expect(resolveHeartSource([moved, { ...device, id: 'hr-other' }], selection)).toBeUndefined();
  expect(resolveHeartSource([{ ...device, name: 'Different strap' }], selection)).toBeUndefined();
});
