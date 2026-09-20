import { test, expect } from 'bun:test';
import { readConfig, overlayUrl, resolveSource, freshSample, type Device } from './config';
test('overlay URLs preserve names and settings without treating names as parameters', () => {
  const config = readConfig('?layout=stack&metrics=heart,power&unit=mph&label=0');
  config.source = 'Steele’s Bike & trainer #1'; config.device = 'ble-123'; config.heartSource = 'Chest & arm'; config.heartDevice = 'hr-123';
  const url = new URL(overlayUrl('http://127.0.0.1:9376', config));
  expect(url.pathname).toBe('/overlay/'); expect(url.searchParams.get('view')).toBe('overlay');
  expect(readConfig(url.search)).toEqual(config);
});
test('invalid customization values fall back to supported choices', () => {
  const config = readConfig('?metrics=nonsense&accent=__proto__&layout=bad&theme=bad');
  expect(config.metrics).toEqual(['power', 'cadence', 'speed']); expect(config.accent).toBe('lime'); expect(config.layout).toBe('bar'); expect(config.theme).toBe('dark');
});
const bike = (id: string): Device => ({ id, name: 'My Bike', connected: true, kind: 'trainer', capabilities: ['power'] });
test('source survives changed daemon IDs but ambiguous names never select the wrong bike', () => {
  const config = { device: 'old-id', source: 'My Bike' };
  expect(resolveSource([bike('new-id')], config)?.id).toBe('new-id');
  expect(resolveSource([bike('new-id'), bike('duplicate')], config)).toBeUndefined();
  expect(resolveSource([bike('old-id'), bike('duplicate')], config)?.id).toBe('old-id');
  expect(resolveSource([bike('new-id')], { device: '', source: 'Missing bike' })).toBeUndefined();
});
test('stale data and disconnected sources are unavailable; actual zeros remain visible', () => {
  const sample = { at: 1000, data: { powerWatts: 0 } };
  expect(freshSample(sample, true, true, 5999)).toEqual({ powerWatts: 0 });
  expect(freshSample(sample, true, true, 6000)).toBeUndefined();
  expect(freshSample(sample, false, true, 1001)).toBeUndefined();
  expect(freshSample(sample, true, false, 1001)).toBeUndefined();
  expect(freshSample({ at: 2000, data: {} }, true, true, 2001)?.powerWatts).toBeUndefined();
});
