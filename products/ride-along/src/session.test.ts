import { test, expect } from 'bun:test';
import { RideSession, duration } from './session';

test('pause and resume exclude paused time and power', () => {
  const ride = new RideSession();
  ride.sample(0, 100); ride.toggle(0); ride.toggle(1000);
  ride.sample(2000, 500); ride.toggle(3000); ride.advance(4000);
  expect(ride.elapsedMs).toBe(2000);
  expect(ride.averageWatts).toBe(300);
});
test('stale readings expire even when the window sleeps; gaps do not become zero watts', () => {
  const ride = new RideSession();
  ride.toggle(0); ride.sample(0, 200); ride.advance(30000);
  ride.sample(30000, 0); ride.advance(35000);
  expect(ride.elapsedMs).toBe(35000);
  expect(ride.averageWatts).toBe(100);
});
test('missing readings and disconnects invalidate power immediately', () => {
  const ride = new RideSession();
  ride.toggle(0); ride.sample(0, 100); ride.sample(1000); ride.advance(9000);
  expect(ride.averageWatts).toBe(100);
  ride.sample(9000, NaN); ride.advance(10000);
  expect(ride.averageWatts).toBe(100);
  ride.reset(10000);
  expect(ride.running).toBe(false); expect(ride.elapsedMs).toBe(0); expect(ride.averageWatts).toBeUndefined();
});
test('power average is time weighted across unequal sample rates', () => {
  const ride = new RideSession(); ride.toggle(0); ride.sample(0, 100);
  ride.sample(1000, 200); ride.sample(1500, 200); ride.advance(2000);
  expect(ride.averageWatts).toBe(150);
  expect(duration(3661000)).toBe('1:01:01'); expect(duration(0)).toBe('00:00');
});
