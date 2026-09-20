<script lang="ts">
  import { onMount } from 'svelte';
  import Overlay from './Overlay.svelte';
  import { isHeartSource, resolveHeartSource, readHeartRate } from '../../shared/heart-rate';
  import { readConfig, overlayUrl, resolveSource, freshSample, metrics, accents, type Metric, type Config, type Point } from './config';
  import { Feed, emptyFeed } from './feed';
  const dashboardUrl = import.meta.env.DEV ? 'http://127.0.0.1:9376/' : '/';
  const overlayOnly = new URLSearchParams(location.search).get('view') === 'overlay';
  let config = $state(readConfig(location.search));
  let feed = $state(emptyFeed());
  let now = $state(performance.now());
  let samples = $state<Point[]>([]);
  let samplePreview = $state(false);
  let copied = $state(false);
  let copyError = $state('');
  let source = $derived(resolveSource(feed.devices, config));
  let bikeData = $derived(freshSample(source ? feed.samples[source.id] : undefined, feed.online, !!source?.connected, now));
  let heartSelection = $derived({ id: config.heartDevice, name: config.heartSource });
  let heartSource = $derived(resolveHeartSource(feed.devices, heartSelection));
  let heartCandidates = $derived(feed.devices.filter(isHeartSource));
  let data = $derived.by(() => {
    const heartRateBpm = readHeartRate(feed.devices, feed.samples, heartSelection, now, feed.online, bikeData?.heartRateBpm);
    return bikeData || heartRateBpm !== undefined ? { ...bikeData, heartRateBpm } : undefined;
  });
  let status = $derived(!feed.online ? 'OFFLINE' : !data ? 'WAITING' : feed.mode === 'demo' ? 'DEMO' : feed.mode === 'replay' ? 'REPLAY' : 'LIVE');
  let url = $derived(overlayUrl(location.origin, config));
  let name = $derived(source?.name ?? (config.source || 'Your bike'));
  let shownData = $derived(samplePreview && !overlayOnly ? { powerWatts: 235, cadenceRpm: 88, speedKph: 32.4, heartRateBpm: 142 } : data);
  let size = $derived(config.layout === 'bar' ? { width: 960, height: 260 } : { width: 360, height: 620 });
  let sampleGraph = $derived(Array.from({ length: 60 }, (_, i) => ({ at: now - (59 - i) * 1000, value: 220 + Math.sin(i / 6) * 30 + Math.cos(i / 3) * 8 })));
  const labels: Record<Metric, string> = { power: 'Power', cadence: 'Cadence', speed: 'Speed', heart: 'Heart rate' };
  let historyKey = $derived(`${source?.id ?? ''}:${feed.epoch}:${source?.connected}`);
  $effect(() => { void historyKey; samples = []; });
  function choose(id: string) {
    const device = feed.devices.find(item => item.id === id);
    config.device = device?.id ?? ''; config.source = device?.name ?? '';
  }
  function chooseHeart(id: string) {
    const device = feed.devices.find(item => item.id === id);
    config.heartDevice = device?.id ?? ''; config.heartSource = device?.name ?? '';
    if (device && !config.metrics.includes('heart')) config.metrics = [...config.metrics, 'heart'];
  }
  function toggleMetric(metric: Metric, checked: boolean) {
    const chosen = new Set(config.metrics);
    if (checked) chosen.add(metric); else if (chosen.size > 1) chosen.delete(metric);
    config.metrics = metrics.filter(item => chosen.has(item));
  }
  async function copy() {
    try { await navigator.clipboard.writeText(url); copied = true; copyError = ''; setTimeout(() => copied = false, 2500); }
    catch { copyError = 'Select the URL below and copy it manually.'; }
  }
  onMount(() => {
    const connection = new Feed(state => feed = state); connection.start();
    const clock = setInterval(() => now = performance.now(), 250);
    const chart = setInterval(() => { const at = performance.now(); samples = [...samples.filter(point => at - point.at < 60000), { at, value: freshSample(source ? feed.samples[source.id] : undefined, feed.online, !!source?.connected, at)?.powerWatts }]; }, 1000);
    return () => { connection.stop(); clearInterval(clock); clearInterval(chart); };
  });
</script>

{#if overlayOnly}
  <div class="broadcast"><Overlay {config} {data} {name} {status} points={samples} {now}/></div>
{:else}
  <div class="setup-shell">
    <header class="topbar"><a class="logo" href={dashboardUrl}>ϟ <span>bikebridge<span class="logo-dot">.</span></span></a><span class="product-tag">STREAM OVERLAY</span><a class="dashboard-link" href={dashboardUrl}>Open dashboard ↗</a></header>
    <main class="setup-main">
      <div class="intro"><div><span class="kicker">YOUR RIDE. ON AIR.</span><h1>Bring your ride to the stream.</h1><p>Live cycling stats, wherever your audience is watching.</p></div><span class="connection" class:online={feed.online}><i></i>{feed.online ? 'BikeBridge connected' : 'Connecting to BikeBridge'}</span></div>
      <div class="workspace">
        <aside class="controls">
          <section><div class="section-label"><span>01</span><h2>Choose your bike</h2></div><label for="bike">Telemetry source</label><select id="bike" value={config.device} onchange={event => choose(event.currentTarget.value)}><option value="">Automatic · connected power source</option>{#if config.device && !feed.devices.some(device => device.id === config.device)}<option value={config.device}>{config.source || 'Saved device'} · unavailable</option>{/if}{#each feed.devices.filter(device => !['bike_controller', 'heart_rate_monitor'].includes(device.kind)) as device}<option value={device.id}>{device.name}{device.connected ? '' : ' · disconnected'}</option>{/each}</select><p class="hint">Connect your bike in the <a href={dashboardUrl}>dashboard</a>. Choose a specific source to keep the same bike after a restart.</p></section>
          <section><label for="heart-source">Heart-rate source</label><select id="heart-source" value={heartSource?.id ?? config.heartDevice} onchange={event => chooseHeart(event.currentTarget.value)}><option value="">Use bike heart rate</option>{#if config.heartDevice && !heartSource}<option value={config.heartDevice}>{config.heartSource || 'Saved monitor'} · unavailable</option>{/if}{#each heartCandidates as device}<option value={device.id}>{device.name}{device.connected ? '' : ' · disconnected'}</option>{/each}</select><p class="hint">Connect your monitor in the dashboard, then select it here to combine its BPM with your bike’s power and speed.</p></section>
          <section><div class="section-label"><span>02</span><h2>Make it yours</h2></div><span class="field-label">Layout</span><div class="choices"><button class:selected={config.layout === 'bar'} onclick={() => config.layout = 'bar'} aria-pressed={config.layout === 'bar'}><span class="layout-icon">▤</span>Horizontal</button><button class:selected={config.layout === 'stack'} onclick={() => config.layout = 'stack'} aria-pressed={config.layout === 'stack'}><span class="layout-icon">▥</span>Stacked</button></div><span class="field-label">Metrics</span><div class="metric-options">{#each metrics as metric}<label><input type="checkbox" checked={config.metrics.includes(metric)} disabled={config.metrics.length === 1 && config.metrics.includes(metric)} onchange={event => toggleMetric(metric, event.currentTarget.checked)}/>{labels[metric]}</label>{/each}</div><div class="field-row"><div><label for="theme">Panel</label><select id="theme" bind:value={config.theme}><option value="dark">Dark</option><option value="light">Light</option></select></div><div><label for="unit">Speed unit</label><select id="unit" bind:value={config.unit}><option value="kph">km/h</option><option value="mph">mph</option></select></div></div><span class="field-label">Accent</span><div class="swatches">{#each Object.entries(accents) as [key, color]}<button style={`--swatch:${color}`} class:chosen={config.accent === key} onclick={() => config.accent = key as Config['accent']} aria-label={`${key} accent`} aria-pressed={config.accent === key} title={key}></button>{/each}</div><div class="toggles"><label><input type="checkbox" bind:checked={config.label}/>Show bike name</label><label><input type="checkbox" bind:checked={config.graph}/>Show power graph</label></div></section>
        </aside>
        <div class="right-column">
          <section class="preview-panel"><div class="preview-toolbar"><div><span class="preview-dot"></span><h2>Preview</h2><span class="preview-size">{size.width} × {size.height}</span></div><label><input type="checkbox" bind:checked={samplePreview}/>Sample data</label></div><div class="preview-canvas" class:tall={config.layout === 'stack'}><div class="preview-overlay" class:stacked={config.layout === 'stack'}><Overlay {config} data={shownData} {name} status={samplePreview ? 'SAMPLE' : status} points={samplePreview ? sampleGraph : samples} {now}/></div><span class="transparency-label">CHECKERBOARD = TRANSPARENT</span></div><div class="preview-note"><span>{samplePreview ? 'Sample data is only shown in this preview.' : data ? 'Live telemetry from your selected bike.' : 'No fresh readings. Connect your bike and start pedaling.'}</span><span>OBS BROWSER SOURCE</span></div></section>
          <section class="install"><div class="install-heading"><div class="section-label"><span>03</span><h2>Add to your stream</h2></div><button class="copy" onclick={copy}>{copied ? 'Copied ✓' : 'Copy overlay URL'}</button></div><label class="url-label" for="overlay-url">Browser Source URL</label><input id="overlay-url" class="url" readonly value={url} onclick={event => event.currentTarget.select()}/>{#if copyError}<p role="alert" class="hint">{copyError}</p>{/if}<ol><li>In OBS, add a <strong>Browser</strong> source.</li><li>Paste the URL. Set width to <strong>{size.width}</strong> and height to <strong>{size.height}</strong>.</li><li>Keep BikeBridge running. Position the overlay over your scene.</li></ol><p class="install-note">The background is transparent. Update the OBS URL after changing these settings. OBS and BikeBridge must run on the same computer.</p><a class="open-overlay" href={url} target="_blank" rel="noreferrer">Open overlay ↗</a></section>
          {#if feed.error}<p class="feed-error" role="alert">{feed.error}</p>{/if}
        </div>
      </div>
      <footer>BIKEBRIDGE <span>Made for the ride. Ready for the stream.</span><span>Local telemetry · No account needed</span></footer>
    </main>
  </div>
{/if}
