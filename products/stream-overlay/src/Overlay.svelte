<script lang="ts">
  import { accents, type Config, type Telemetry, type Point } from './config';
  let { config, data, name, status, now, points = [] }: { config: Config; data?: Telemetry; name: string; status: string; now: number; points?: Point[] } = $props();
  const titles = { power: 'POWER', cadence: 'CADENCE', speed: 'SPEED', heart: 'HEART RATE' };
  let values = $derived({ power: data?.powerWatts, cadence: data?.cadenceRpm, speed: data?.speedKph === undefined ? undefined : data.speedKph * (config.unit === 'mph' ? 0.621371 : 1), heart: data?.heartRateBpm });
  let units = $derived({ power: 'W', cadence: 'rpm', speed: config.unit === 'mph' ? 'mph' : 'km/h', heart: 'bpm' });
  let path = $derived.by(() => {
    const max = Math.max(200, ...points.map(point => Math.max(0, point.value ?? 0)));
    let result = '', gap = true;
    for (let i = 0; i < points.length; i++) {
      const { at, value } = points[i];
      if (value === undefined || now - at > 60000) { gap = true; continue; }
      if (i > 0 && at - points[i - 1].at > 2000) gap = true;
      const x = Math.max(0, 1 - (now - at) / 60000) * 900;
      result += `${gap ? 'M' : 'L'}${x.toFixed(1)},${(43 - Math.max(0, value) / max * 39).toFixed(1)} `;
      gap = false;
    }
    return result;
  });
  function format(value: number | undefined, decimal = false) { return value !== undefined && Number.isFinite(value) ? value.toFixed(decimal ? 1 : 0) : '—'; }
</script>
<section class="overlay-card" class:stack={config.layout === 'stack'} class:light={config.theme === 'light'} style={`--accent:${accents[config.accent]}`} aria-label="Cycling stream overlay">
  <div class="overlay-heading"><div class="overlay-source"><span class="bolt">ϟ</span>{#if config.label}<span>{name}</span>{:else}<span class="wordmark">BIKEBRIDGE</span>{/if}</div><span class="overlay-status" class:waiting={status !== 'LIVE'}><i></i>{status}</span></div>
  <div class="overlay-metrics">
    {#each config.metrics as metric}<div class="overlay-metric" class:power={metric === 'power'}><span class="overlay-caption">{titles[metric]}</span><div class="overlay-value">{format(values[metric], metric === 'speed')}<span>{units[metric]}</span></div></div>{/each}
  </div>
  {#if config.graph}<div class="overlay-chart"><span>POWER · 60 SEC</span><svg viewBox="0 0 900 48" preserveAspectRatio="none" role="img" aria-label="Power over the last 60 seconds"><path class="baseline" d="M0 43H900"/><path d={path}/></svg></div>{/if}
</section>
<style>
  .overlay-card{--ink:#f3f6ef;--subtle:#adbbb2;--line:#e0f4e618;color:var(--ink);background:#14201ee8;border:1px solid #c4e5d326;border-radius:18px;box-shadow:0 8px 22px #0003;padding:18px 25px 15px;font-family:Inter,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;overflow:hidden;width:100%;font-variant-numeric:tabular-nums}
  .light{--ink:#16251b;--subtle:#526458;--line:#1f3a2520;background:#f1f5edee;border-color:#ffffff70}.overlay-heading{display:flex;align-items:center;justify-content:space-between;gap:16px;margin-bottom:17px}.overlay-source{display:flex;align-items:center;gap:9px;min-width:0;font-size:13px;font-weight:500}.overlay-source>span:last-child{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.bolt{color:var(--accent);font-size:22px;line-height:1;font-weight:700}.light .bolt{color:var(--ink)}.wordmark{font-size:10px;letter-spacing:2px}.overlay-status{display:flex;align-items:center;gap:6px;font-size:9px;font-weight:600;letter-spacing:1.5px;flex-shrink:0;color:var(--accent)}.light .overlay-status{color:#285134}.overlay-status.waiting{color:var(--subtle)}.overlay-status i{display:block;width:5px;height:5px;border-radius:50%;background:currentColor}.overlay-metrics{display:flex;gap:0}.overlay-metric{flex:1;min-width:0;border-left:1px solid var(--line);padding:0 25px}.overlay-metric:first-child{border-left:none;padding-left:0}.overlay-metric:last-child{padding-right:0}.overlay-caption{font-size:9px;letter-spacing:2px;color:var(--subtle);font-weight:500}.overlay-value{font-size:51px;letter-spacing:-2px;line-height:1.25;margin-top:5px;white-space:nowrap;font-weight:560}.overlay-value>span{font-size:13px;letter-spacing:0;color:var(--subtle);margin-left:7px;font-weight:400}.power .overlay-value{color:var(--accent)}.light .power .overlay-value{color:var(--ink)}.overlay-chart{position:relative;margin-top:14px;padding-top:4px;border-top:1px solid var(--line)}.overlay-chart>span{position:absolute;left:0;top:6px;color:var(--subtle);font-size:7px;letter-spacing:1.6px}.overlay-chart svg{width:100%;height:43px;overflow:visible}.overlay-chart path{fill:none;stroke:var(--accent);stroke-width:2;vector-effect:non-scaling-stroke;stroke-linejoin:round;stroke-linecap:round}.light .overlay-chart path{stroke:color-mix(in srgb,var(--accent) 45%,#193020)}.overlay-chart .baseline{stroke:var(--line);stroke-width:1}.stack{padding:23px}.stack .overlay-heading{margin-bottom:19px}.stack .overlay-metrics{flex-direction:column}.stack .overlay-metric{border-left:0;border-top:1px solid var(--line);padding:14px 0 12px;display:flex;align-items:center;justify-content:space-between;gap:10px}.stack .overlay-metric:first-child{border-top:0;padding-top:0}.stack .overlay-caption{font-size:8px;letter-spacing:1.2px}.stack .overlay-value{font-size:39px;letter-spacing:-1.5px;margin:0}.stack .power{display:block}.stack .power .overlay-value{font-size:75px;margin-top:3px}.stack .overlay-chart{margin-top:8px}.stack .overlay-chart svg{height:62px}
  @media(max-width:650px){.overlay-card:not(.stack) .overlay-metric{padding:0 12px}.overlay-card:not(.stack) .overlay-metric:first-child{padding-left:0}.overlay-card:not(.stack) .overlay-value{font-size:32px}.overlay-card:not(.stack) .overlay-value>span{font-size:10px;margin-left:4px}.overlay-card:not(.stack) .overlay-caption{font-size:8px;letter-spacing:1px}}
</style>
