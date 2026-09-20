<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
  import Icon from './Icon.svelte';
  import { isHeartSource, resolveHeartSource, readHeartRate, type HeartSample } from '../../shared/heart-rate';
  import { native, snapshot, connectDevice, ensureService, openProduct, Stream, type Device, type Telemetry, type Envelope, type Connection } from './client';
  import { RideSession, FRESH_MS, duration } from './session';

  function saved() {
    try { const value = JSON.parse(localStorage.getItem('ride-along.settings') ?? '{}'); return value && typeof value === 'object' ? value : {}; } catch { return {}; }
  }
  const preferences = saved();
  const initialPort: number = Number.isInteger(preferences.port) && preferences.port > 0 && preferences.port <= 65535 ? preferences.port : 9376;
  let port = $state(initialPort);
  let portInput = $state<number | undefined>(initialPort);
  let preferredName = $state(typeof preferences.source === 'string' ? preferences.source : '');
  let heartSelection = $state({ id: typeof preferences.heartDevice === 'string' ? preferences.heartDevice : '', name: typeof preferences.heartSource === 'string' ? preferences.heartSource : '' });
  let heartSamples = $state<Record<string, HeartSample>>({});
  let pinned = $state(true);
  let compact = $state(false);
  let settings = $state(!preferences.source);
  let serviceOwned = $state(false);
  let connection = $state<Connection>('connecting');
  let devices = $state<Device[]>([]);
  let selectedId = $state('');
  let demo = $state(false);
  let replay = $state(false);
  let reading = $state<{ at: number; data: Telemetry }>();
  let now = $state(performance.now());
  let error = $state('');
  let busy = $state(false);
  let running = $state(false);
  let elapsed = $state(0);
  let average = $state<number>();
  let history = $state<{ at: number; value?: number }[]>([]);
  const ride = new RideSession(performance.now());
  let stream: Stream | undefined;
  let generation = 0;
  let revision = 0;
  let disposed = false;
  let refreshing = false;
  let lastChart = 0;
  let source = $derived(devices.find(device => device.id === selectedId));
  let candidates = $derived(devices.filter(device => !['bike_controller', 'heart_rate_monitor'].includes(device.kind)));
  let fresh = $derived(connection === 'online' && !!source?.connected && !!reading && now - reading.at < FRESH_MS);
  let heartCandidates = $derived(devices.filter(isHeartSource));
  let heartSource = $derived(resolveHeartSource(devices, heartSelection));
  let externalHeart = $derived(!!(heartSelection.id || heartSelection.name));
  let data = $derived.by(() => {
    const bikeData = fresh ? reading?.data : undefined;
    const heartRateBpm = readHeartRate(devices, heartSamples, heartSelection, now, connection === 'online', bikeData?.heartRateBpm);
    return bikeData || heartRateBpm !== undefined ? { ...bikeData, heartRateBpm } : undefined;
  });
  let label = $derived(connection !== 'online' ? 'Bridge offline' : fresh ? 'Live' : source?.connected ? 'Waiting for data' : 'No bike connected');
  let plot = $derived.by(() => {
    const visible = history.filter(point => now - point.at <= 60000);
    const max = Math.max(200, ...visible.map(point => Math.abs(point.value ?? 0)));
    let path = ''; let gap = true;
    for (const point of visible) {
      if (point.value === undefined) { gap = true; continue; }
      const x = Math.max(0, 320 * (1 - (now - point.at) / 60000));
      const y = 61 - Math.max(0, point.value) / max * 54;
      path += `${gap ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)} `; gap = false;
    }
    return path;
  });
  function persist() {
    try { localStorage.setItem('ride-along.settings', JSON.stringify({ port, source: preferredName, heartDevice: heartSelection.id, heartSource: heartSelection.name })); } catch { /* Window remains usable without storage. */ }
  }
  function clearReading() { reading = undefined; ride.sample(performance.now()); }
  function choose(id: string, remember = false) {
    if (selectedId !== id) { selectedId = id; clearReading(); history = []; }
    if (remember) { preferredName = devices.find(device => device.id === id)?.name ?? ''; persist(); }
  }
  function chooseHeart(id: string) {
    const device = devices.find(item => item.id === id);
    heartSelection = { id: device?.id ?? '', name: device?.name ?? '' }; persist();
  }
  function reconcile() {
    if (devices.some(device => device.id === selectedId)) return;
    const named = devices.filter(device => device.name === preferredName);
    const next = preferredName ? (named.length === 1 ? named[0] : undefined)
      : devices.find(device => device.connected && device.capabilities.includes('power'))
        ?? candidates.find(device => device.capabilities.includes('power'));
    choose(next?.id ?? '');
  }
  async function refresh(token = generation) {
    if (refreshing) return;
    refreshing = true;
    const startRevision = revision;
    try {
      const next = await snapshot(port);
      if (token !== generation || disposed || connection !== 'online') return;
      if (next.status.protocolVersion !== 1) throw new Error('This BikeBridge protocol version is unsupported.');
      demo = next.status.mockMode; replay = !!next.status.replay;
      if (revision === startRevision) { devices = next.devices; reconcile(); if (!source?.connected) clearReading(); }
    } catch (reason) { if (token === generation && connection === 'online') error = String(reason); }
    finally { refreshing = false; }
  }
  function message(event: Envelope) {
    if (event.type === 'telemetry' && event.deviceId) {
      const at = performance.now(); now = at;
      heartSamples[event.deviceId] = { at, data: event.data };
      if (event.deviceId === selectedId) {
        reading = { at, data: event.data }; ride.sample(at, event.data.powerWatts);
      }
    } else if (['device.discovered', 'device.updated', 'device.connected', 'device.disconnected'].includes(event.type)) {
      const device = event.data as Device;
      revision++; devices = [...devices.filter(item => item.id !== device.id), device]; reconcile();
      if (!device.connected) delete heartSamples[device.id];
      if (device.id === selectedId && !device.connected) clearReading();
    } else if (event.type === 'replay.reset') {
      clearReading(); heartSamples = {}; history = []; void refresh();
    } else if (event.type === 'error') {
      error = event.data?.message ?? 'BikeBridge reported an error.';
      if (event.data?.code === 'events_lost') { clearReading(); heartSamples = {}; void refresh(); }
    }
  }
  async function startStream() {
    const token = ++generation;
    await stream?.stop();
    if (disposed || token !== generation) return;
    clearReading(); heartSamples = {}; history = []; devices = []; selectedId = ''; connection = 'connecting'; error = '';
    stream = new Stream(port, event => { if (token === generation) message(event); }, state => {
      if (token !== generation) return;
      connection = state;
      if (state === 'online') { error = ''; void refresh(token); }
      else { clearReading(); heartSamples = {}; devices = []; selectedId = ''; }
    });
    try { if (native) serviceOwned = await ensureService(); await stream.start(); } catch (reason) { error = String(reason); connection = 'offline'; }
  }
  async function connect(id: string) {
    if (busy) return;
    busy = true; error = '';
    try { await connectDevice(port, id); await refresh(); } catch (reason) { error = String(reason); }
    finally { busy = false; }
  }
  async function applyPort() {
    const value = Number(portInput);
    if (!Number.isInteger(value) || value < 1 || value > 65535) { error = 'Enter a port from 1 to 65535.'; return; }
    port = value; persist(); busy = true;
    try { await startStream(); } finally { busy = false; }
  }
  async function launchProduct(product: 'dashboard' | 'overlay') {
    try { await openProduct(port, product); } catch (reason) { error = String(reason); }
  }
  async function pin() {
    try { await getCurrentWindow().setAlwaysOnTop(!pinned); pinned = !pinned; } catch (reason) { error = String(reason); }
  }
  async function resize(small: boolean) {
    try {
      if (native) await getCurrentWindow().setSize(new LogicalSize(380, small ? 240 : 520));
      compact = small; settings = false;
    } catch (reason) { error = String(reason); }
  }
  async function windowAction(action: 'close' | 'minimize') {
    try { await getCurrentWindow()[action](); } catch (reason) { error = String(reason); }
  }
  async function drag(event: PointerEvent) {
    if (native && event.button === 0) {
      try { await getCurrentWindow().startDragging(); } catch (reason) { error = String(reason); }
    }
  }
  function syncRide() { running = ride.running; elapsed = ride.elapsedMs; average = ride.averageWatts; }
  function toggleRide() { ride.toggle(performance.now()); syncRide(); }
  function resetRide() { ride.reset(performance.now()); syncRide(); }
  function number(value?: number, decimals = 0) { return value !== undefined && Number.isFinite(value) ? value.toFixed(decimals) : '—'; }
  onMount(() => {
    void startStream();
    const tick = setInterval(() => {
      now = performance.now(); ride.advance(now); syncRide();
      if (now - lastChart >= 1000) {
        lastChart = now; history = [...history.filter(point => now - point.at <= 60000), { at: now, value: data?.powerWatts }];
      }
    }, 250);
    const poll = setInterval(() => { if (connection === 'online') void refresh(); }, 5000);
    return () => { disposed = true; generation++; clearInterval(tick); clearInterval(poll); void stream?.stop(); };
  });
</script>

<div class="app" class:compact>
  <header>
    <div class="brand" role="presentation" onpointerdown={drag}><span class="brand-mark"><Icon name="bolt" size={15} /></span><span>ride along<span class="brand-dot">.</span></span></div>
    <nav aria-label="Window controls">
      <button class="icon-button" class:active={pinned && native} disabled={!native} onclick={pin} title={native ? (pinned ? 'Unpin window' : 'Keep on top') : 'Pin is available in the desktop app'} aria-label={pinned ? 'Unpin window' : 'Keep on top'} aria-pressed={pinned}><Icon name="pin" size={14} /></button>
      <button class="icon-button" class:active={settings} onclick={() => { if (compact) void resize(false).then(() => settings = true); else settings = !settings; }} title="Settings" aria-label="Settings" aria-expanded={settings}><Icon name="settings" size={15} /></button>
      <button class="icon-button" onclick={() => resize(!compact)} title={compact ? 'Expand' : 'Compact mode'} aria-label={compact ? 'Expand' : 'Compact mode'}><Icon name={compact ? 'expand' : 'compact'} size={14} /></button>
      {#if native}<button class="icon-button" onclick={() => windowAction('minimize')} title="Minimize" aria-label="Minimize"><Icon name="minimize" size={14} /></button><button class="icon-button close" onclick={() => windowAction('close')} title="Hide to tray — BikeBridge keeps running" aria-label="Hide to tray"><Icon name="close" size={14} /></button>{/if}
    </nav>
  </header>

  {#if settings}
    <main class="settings">
      <div class="section-heading"><h1>Your setup</h1><span class="eyebrow">{connection === 'online' ? 'BRIDGE CONNECTED' : 'BRIDGE OFFLINE'}</span></div>
      <div class="product-links"><button class="secondary" onclick={() => launchProduct('dashboard')}>Devices &amp; dashboard ↗</button><button class="secondary" onclick={() => launchProduct('overlay')}>Stream overlay ↗</button></div>
      <p class="hint">Everything is included. Connect your devices, then copy your overlay URL into OBS. Closing this window keeps BikeBridge running in the tray.</p>
      <label for="source">Bike / power source</label>
      <select id="source" value={selectedId} onchange={event => choose(event.currentTarget.value, true)} disabled={connection !== 'online'}>
        <option value="">Select a device</option>
        {#each candidates as device}<option value={device.id}>{device.name}{device.connected ? ' · connected' : ''}</option>{/each}
      </select>
      <p class="hint">Devices discovered by BikeBridge appear here. Use the dashboard to scan for a new Bluetooth name.</p>
      {#if source && !source.connected}<button class="primary connect" onclick={() => source && connect(source.id)} disabled={busy || replay}>{busy ? 'Connecting…' : 'Connect device'}</button>{/if}
      <div class="divider"></div>
      <label for="heart-source">Heart-rate source</label>
      <select id="heart-source" value={heartSource?.id ?? heartSelection.id} onchange={event => chooseHeart(event.currentTarget.value)} disabled={connection !== 'online'}>
        <option value="">Use bike heart rate</option>
        {#if externalHeart && !heartSource}<option value={heartSelection.id}>{heartSelection.name || 'Saved monitor'} · unavailable</option>{/if}
        {#each heartCandidates as device}<option value={device.id}>{device.name}{device.connected ? ' · connected' : ''}</option>{/each}
      </select>
      <p class="hint">Use a separate chest strap or armband alongside your bike. Missing or stale monitor readings show a dash.</p>
      {#if heartSource && !heartSource.connected}<button class="primary connect" onclick={() => heartSource && connect(heartSource.id)} disabled={busy || replay}>{busy ? 'Connecting…' : 'Connect monitor'}</button>{/if}
      <div class="divider"></div>
      <details><summary>Advanced connection</summary>
      <label for="port">BikeBridge port <span>on this computer</span></label>
      <form class="port-row" onsubmit={event => { event.preventDefault(); void applyPort(); }}><input id="port" type="number" min="1" max="65535" bind:value={portInput} disabled={!native || busy} /><button class="secondary" disabled={!native || busy}>Apply</button></form>
      <p class="hint">The included service uses port 9376. Change this only to use another local BikeBridge service.</p></details>
      {#if native}<p class="hint">{serviceOwned ? 'BikeBridge starts automatically with this app.' : 'Using an existing BikeBridge service when available.'} Choose Quit BikeBridge from the tray to exit.</p><button class="secondary" disabled={busy} onclick={() => void startStream()}>Retry connection</button>{/if}
      {#if !native}<p class="hint">Browser preview uses the development proxy on port 9376. Window controls work in the desktop app.</p>{/if}
      <div class="settings-footer"><span>BikeBridge / Ride Along</span><span>v0.1.0</span></div>
    </main>
  {:else}
    <main class="ride-view">
      <div class="source-row"><button class="source" onclick={() => { if (compact) void resize(false).then(() => settings = true); else settings = true; }} title="Choose your bike">{source?.name ?? (preferredName || 'Choose your bike')}<span>⌄</span></button><span class="status" class:live={fresh}><i></i>{demo ? 'DEMO' : replay ? 'REPLAY' : fresh ? 'LIVE' : 'IDLE'}</span></div>
      <div class="power-row"><div class="power"><span class="power-value">{number(data?.powerWatts)}</span><span class="power-unit">WATTS</span></div>{#if compact}<div class="compact-metrics"><span><b>{number(data?.cadenceRpm)}</b> rpm</span><span><b>{number(data?.speedKph, 1)}</b> km/h</span>{#if externalHeart || data?.heartRateBpm !== undefined}<span><b>{number(data?.heartRateBpm)}</b> bpm</span>{/if}</div>{:else}<span class="power-symbol"><Icon name="bolt" size={28} /></span>{/if}</div>
      {#if !compact}
        <div class="chart"><div class="chart-heading"><span>POWER</span><span>LAST 60 SEC</span></div><svg viewBox="0 0 320 68" preserveAspectRatio="none" aria-label="Power over the last 60 seconds" role="img"><path class="grid" d="M0 7H320M0 34H320M0 61H320"/><path class="trace" d={plot}/></svg></div>
        <div class="metrics"><div><span class="metric-label"><Icon name="cadence" size={13}/>CADENCE</span><p>{number(data?.cadenceRpm)}<span>rpm</span></p></div><div><span class="metric-label"><Icon name="speed" size={13}/>SPEED</span><p>{number(data?.speedKph, 1)}<span>km/h</span></p></div>{#if externalHeart || data?.heartRateBpm !== undefined}<div><span class="metric-label"><Icon name="heart" size={13}/>HEART</span><p>{number(data?.heartRateBpm)}<span>bpm</span></p></div>{/if}</div>
      {/if}
      <div class="session"><div class="timer"><span class="eyebrow">{running ? 'RIDING' : elapsed > 0 ? 'PAUSED' : 'RIDE TIME'}</span><span class="time">{duration(elapsed)}</span></div>{#if !compact}<div class="average"><span class="eyebrow">AVG POWER</span><span>{number(average)}<small> W</small></span></div>{/if}<div class="session-buttons"><button class="icon-button reset" onclick={resetRide} disabled={elapsed === 0} title="Reset ride timer and average" aria-label="Reset ride"><Icon name="reset" size={15}/></button><button class="ride-button" class:running onclick={toggleRide} title={running ? 'Pause ride timer' : 'Start ride timer'} aria-label={running ? 'Pause ride' : 'Start ride'}><Icon name={running ? 'pause' : 'play'} size={17}/>{#if !compact}<span>{running ? 'Pause' : elapsed > 0 ? 'Resume' : 'Ride'}</span>{/if}</button></div></div>
      {#if !compact}<footer><span class="footer-dot" class:live={fresh}></span><span>{label}</span><span class="footer-right">{native && pinned ? 'PINNED ON TOP' : 'BIKEBRIDGE'}</span></footer>{/if}
    </main>
  {/if}
  {#if error}<div class="notice" role="alert"><span>{error}</span><button onclick={() => error = ''} aria-label="Dismiss error"><Icon name="close" size={12}/></button></div>{/if}
  {#if connection !== 'online' && !settings && !compact}<div class="offline-help">{connection === 'connecting' ? 'Connecting' : 'Reconnecting'} to BikeBridge · port {port}<button onclick={() => settings = true}>Setup</button></div>{/if}
</div>
