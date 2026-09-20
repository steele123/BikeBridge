export const FRESH_MS = 5000;

// Integrate only the portion of a power reading's life that overlaps active riding.
// All times use performance.now(): wall-clock adjustments cannot change a ride.
export class RideSession {
  running = false;
  elapsedMs = 0;
  private energy = 0;
  private coveredMs = 0;
  private last: number;
  private reading?: { watts: number; at: number };
  constructor(now = 0) { this.last = now; }
  advance(now: number) {
    const end = Math.max(now, this.last);
    if (this.running) {
      this.elapsedMs += end - this.last;
      if (this.reading) {
        const duration = Math.max(0, Math.min(end, this.reading.at + FRESH_MS) - this.last);
        this.energy += this.reading.watts * duration;
        this.coveredMs += duration;
      }
    }
    this.last = end;
  }
  sample(now: number, watts?: number) {
    this.advance(now);
    this.reading = watts !== undefined && Number.isFinite(watts) ? { watts, at: now } : undefined;
  }
  toggle(now: number) { this.advance(now); this.running = !this.running; }
  reset(now: number) { this.advance(now); this.running = false; this.elapsedMs = 0; this.energy = 0; this.coveredMs = 0; }
  get averageWatts() { return this.coveredMs > 0 ? this.energy / this.coveredMs : undefined; }
}
export function duration(ms: number) {
  const seconds = Math.floor(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  return minutes >= 60
    ? `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
    : `${String(minutes).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
}
