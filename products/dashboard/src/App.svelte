<script lang="ts">
  import { onMount } from 'svelte';
  import { BridgeClient, http, type Device, type Telemetry, type Status, type Envelope, type ConnectionState } from './client';
  import Icon from './Icon.svelte';
  import Chart from './Chart.svelte';

  let view = $state('overview');
  let connection = $state<ConnectionState>('connecting');
  let status = $state<Status | null>(null);
  let devices = $state<Device[]>([]);
  let selectedId = $state('');
  let query = $state('');
  let deviceName = $state('');
  let showNameForm = $state(false);
  type NearbyDevice = { id: string; name: string | null; signalStrength: number | null; deviceId: string | null };
  let nearby = $state<NearbyDevice[]>([]);
  let showNearby = $state(false);
  let nearbyQuery = $state('');
  let showUnnamed = $state(false);
  let nearbyLoading = $state(false);
  let nearbyError = $state('');
  let visibleNearby = $derived(nearby.filter(device => (showUnnamed || device.name) &&
    (device.name ?? 'Unnamed device').toLowerCase().includes(nearbyQuery.trim().toLowerCase())));
  let samples = $state<Record<string, { data: Telemetry; at: number }>>({});
  let history = $state<Record<string, { at: number; value?: number }[]>>({});
  let logs = $state<{ id: number; at: number; event: Envelope }[]>([]);
  let logSequence = 0;
  let now = $state(Date.now());
  let busy = $state('');
  let notice = $state<{ message: string; error: boolean } | null>(null);
  let ownerId = $state('');
  let inputs = $state<Record<string, Record<string, { label: string; state: string; value?: number; at: number }>>>({});
  let controllerInputs = $derived(Object.values(inputs[selectedId] ?? {}));
  function inputLabel(data: { input: string; button?: number }) {
    if (data.input === 'button') return ({ 8193: 'Up', 8194: 'Down', 8195: 'Y', 8196: 'Z' } as Record<number, string>)[data.button ?? -1] ?? `Button ${data.button}`;
    return data.input.replaceAll('_', ' ');
  }
  let watts = $state(150);
  let resistance = $state(20);
  let grade = $state(0);
  let includeTelemetry = $state(false);
  let apiMethod = $state('GET');
  let apiPath = $state('/api/status');
  let apiBody = $state('{\n  "type": "device.connect",\n  "deviceId": ""\n}');
  let apiResult = $state('Send a request to see its response here.');
  let client: BridgeClient;
  let refreshRunning = false;
  let selected = $derived(devices.find(device => device.id === selectedId));
  let online = $derived(connection === 'online');
  let latest = $derived(samples[selectedId]);
  let fresh = $derived(online && !!selected?.connected && !!latest && now - latest.at < 5000);
  let filteredDevices = $derived(devices.filter(d => `${d.name} ${d.kind}`.toLowerCase().includes(query.toLowerCase())));
  let filteredLogs = $derived(logs.filter(log => includeTelemetry || log.event.type !== 'telemetry'));
  let controllable = $derived(selected?.capabilities.some(c => ['erg_control', 'resistance_control', 'simulation_control'].includes(c)) ?? false);
  let controlsEnabled = $derived(online && !!selected?.connected && ownerId === selectedId && !busy && !status?.replay);
  const metrics = [
    { key: 'powerWatts', name: 'Power', unit: 'W', icon: 'bolt' },
    { key: 'cadenceRpm', name: 'Cadence', unit: 'rpm', icon: 'cadence' },
    { key: 'speedKph', name: 'Speed', unit: 'km/h', icon: 'speed' },
    { key: 'heartRateBpm', name: 'Heart rate', unit: 'bpm', icon: 'heart' },
  ] as const;

  function log(event: Envelope) { logs = [{ id: ++logSequence, at: Date.now(), event }, ...logs].slice(0, 200); }
  function upsert(device: Device) {
    devices = [...devices.filter(d => d.id !== device.id), device].sort((a, b) => a.name.localeCompare(b.name));
    if (!selectedId) selectedId = device.id;
  }
  function message(event: Envelope) {
    log(event);
    if (event.direction === 'out') return;
    if (event.type === 'telemetry' && event.deviceId) {
      const at = Date.now();
      samples[event.deviceId] = { data: event.data, at };
      history[event.deviceId] = [...(history[event.deviceId] ?? []), { at, value: event.data.powerWatts }].slice(-300);
    } else if (event.type === 'input' && event.deviceId) {
      const key = `${event.data.input}:${event.data.button ?? ''}`;
      inputs[event.deviceId] = { ...(inputs[event.deviceId] ?? {}), [key]: { label: inputLabel(event.data), state: event.data.state, value: event.data.value, at: Date.now() } };
    } else if (['device.discovered', 'device.updated', 'device.connected', 'device.disconnected'].includes(event.type)) {
      upsert(event.data);
      if (!event.data.connected) { delete samples[event.data.id]; delete inputs[event.data.id]; if (ownerId === event.data.id) ownerId = ''; }
    } else if (event.type === 'scan.status' && status) {
      status.scan = event.data;
    } else if (event.type === 'error') {
      notice = { message: event.data?.message ?? 'BikeBridge reported an error.', error: true };
      if (event.data?.code === 'events_lost') { inputs = {}; void refresh(); }
    } else if (event.type === 'replay.reset') {
      samples = {}; history = {}; inputs = {}; void refresh();
    }
  }
  async function refresh() {
    if (refreshRunning) return;
    refreshRunning = true;
    try {
      const [nextStatus, nextDevices] = await Promise.all([http('/api/status'), http('/api/devices')]);
      status = nextStatus; devices = nextDevices;
      if (!devices.some(d => d.id === selectedId)) {
        selectedId = devices.find(d => d.connected && d.capabilities.includes('power'))?.id ?? devices[0]?.id ?? '';
      }
    } catch (error) {
      if (online) notice = { message: error instanceof Error ? error.message : String(error), error: true };
    } finally { refreshRunning = false; }
  }
  async function refreshNearby() {
    if (nearbyLoading) return;
    nearbyLoading = true;
    try { nearby = await http('/api/scan/nearby'); nearbyError = ''; }
    catch (error) { nearbyError = error instanceof Error ? error.message : String(error); }
    finally { nearbyLoading = false; }
  }
  function browseBluetooth() {
    showNearby = !showNearby;
    if (showNearby) void action('browse', async () => {
      if (!status?.scan.scanning) await http('/api/scan/start', 'POST');
      await refreshNearby();
    });
  }
  function selectNearby(device: NearbyDevice) {
    if (device.deviceId) { selectedId = device.deviceId; showNearby = false; return; }
    return action(device.id, async () => {
      await http(`/api/scan/nearby/${encodeURIComponent(device.id)}/select`, 'POST');
      await refreshNearby();
      const added = nearby.find(item => item.id === device.id);
      if (added?.deviceId) { selectedId = added.deviceId; showNearby = false; }
    }, 'Device added. Select Connect to check its supported services.');
  }
  async function action(key: string, operation: () => Promise<unknown>, success?: string) {
    if (busy) return;
    busy = key; notice = null;
    try {
      await operation();
      if (success) notice = { message: success, error: false };
      await refresh();
    } catch (error) { notice = { message: error instanceof Error ? error.message : String(error), error: true }; }
    finally { busy = ''; }
  }
  function deviceAction(device: Device) {
    selectedId = device.id;
    return action(device.id, async () => {
      await client.send({ type: device.connected ? 'device.disconnect' : 'device.connect', deviceId: device.id });
      if (device.connected && ownerId === device.id) ownerId = '';
    });
  }
  function control(type: string, data?: unknown) {
    const id = selectedId;
    return action(type, async () => {
      const result = await client.send({ type, deviceId: id, ...(data === undefined ? {} : { data }) });
      if (type === 'trainer.requestControl') ownerId = id;
      if (type === 'trainer.reset') ownerId = '';
      log({ type: 'control.applied', data: result });
    }, 'Command applied.');
  }
  async function findByName(event: SubmitEvent) {
    event.preventDefault();
    await action('name', async () => {
      const name = deviceName.trim();
      await http('/api/scan/select', 'POST', { name });
      log({ type: 'scan.name_selected', data: { name } });
      deviceName = ''; showNameForm = false;
    }, 'Scanning for that name. Keep the device awake; connect it when it appears below.');
  }
  function format(value: number | undefined, digits = 0) { return value === undefined ? '—' : value.toFixed(digits); }
  function kindName(kind: string) { return kind.replaceAll('_', ' '); }
  function time(at: number) { return new Date(at).toLocaleTimeString([], { hour12: false }); }
  function logDetail(event: Envelope) { if (event.type === 'input' && event.data) return `${inputLabel(event.data)} · ${event.data.state}${event.data.value === undefined ? '' : ` · ${event.data.value}`}`; return event.data?.name ?? event.error?.message ?? event.data?.message ?? event.deviceId ?? (event.success === undefined ? 'BikeBridge' : event.success ? 'Request succeeded' : 'Request failed'); }
  function preset(kind: string) {
    if (kind === 'status') { apiMethod = 'GET'; apiPath = '/api/status'; }
    if (kind === 'devices') { apiMethod = 'GET'; apiPath = '/api/devices'; }
    if (kind === 'subscribe') { apiMethod = 'WS'; apiBody = JSON.stringify({ type: 'subscribe', events: ['telemetry', 'device', 'scan', 'input', 'error', 'command', 'replay', 'session'] }, null, 2); }
    if (kind === 'connect') { apiMethod = 'WS'; apiBody = JSON.stringify({ type: 'device.connect', deviceId: selectedId }, null, 2); }
    if (kind === 'name') { apiMethod = 'POST'; apiPath = '/api/scan/select'; apiBody = JSON.stringify({ name: "Steele's Bike" }, null, 2); }
  }
  function sendApi() {
    return action('api', async () => {
      try {
        let result: unknown;
        if (apiMethod === 'WS') {
          const body = JSON.parse(apiBody);
          if (!body || typeof body !== 'object' || Array.isArray(body) || typeof body.type !== 'string') throw new Error('Enter a JSON object with a type field.');
          result = await client.send(body);
        } else {
          if (!apiPath.startsWith('/api/') || apiPath.includes('\\')) throw new Error('Use a local path starting with /api/.');
          const body = apiMethod === 'POST' && apiBody.trim() ? JSON.parse(apiBody) : undefined;
          log({ type: `http.${apiMethod.toLowerCase()}`, data: { path: apiPath, body }, direction: 'out' });
          result = await http(apiPath, apiMethod, body);
        }
        apiResult = JSON.stringify(result, null, 2);
      } catch (error) {
        apiResult = JSON.stringify({ error: error instanceof Error ? error.message : String(error) }, null, 2);
        throw error;
      }
    });
  }
  function exportLog() {
    const url = URL.createObjectURL(new Blob([JSON.stringify(logs, null, 2)], { type: 'application/json' }));
    const anchor = document.createElement('a'); anchor.href = url; anchor.download = 'bikebridge-events.json'; anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  onMount(() => {
    client = new BridgeClient(message, state => {
      connection = state;
      if (state === 'online') { samples = {}; history = {}; inputs = {}; void refresh(); }
      else { ownerId = ''; inputs = {}; nearby = []; }
    });
    client.start(); void refresh();
    const timer = setInterval(() => { now = Date.now(); }, 1000);
    const polling = setInterval(() => { void refresh(); if (showNearby && online) void refreshNearby(); }, 3000);
    return () => { clearInterval(timer); clearInterval(polling); client.stop(); };
  });
</script>

<svelte:head><title>BikeBridge · {view === 'overview' ? 'Overview' : 'API console'}</title></svelte:head>

<div class="app-shell">
  <aside class="sidebar">
    <a class="brand" href="/" aria-label="BikeBridge home"><span class="brand-symbol"><Icon name="bridge" size={25} /></span><span>bikebridge<span class="brand-dot">.</span></span></a>
    <div class="workspace-label">YOUR LOCAL WORKSPACE</div>
    <nav aria-label="Main navigation">
      <button class:active={view === 'overview'} onclick={() => view = 'overview'}><Icon name="grid" /><span>Overview</span><span class="nav-shortcut">01</span></button>
      <button class:active={view === 'api'} onclick={() => view = 'api'}><Icon name="terminal" /><span>API console</span><span class="nav-shortcut">02</span></button>
    </nav>
    <div class="sidebar-devices"><div class="workspace-label">CONNECTED DEVICES <span>{devices.filter(d => d.connected).length}</span></div>
      {#each devices.filter(d => d.connected) as device}
        <button onclick={() => { selectedId = device.id; view = 'overview'; }} class:chosen={selectedId === device.id}><span class="signal-dot"></span><span>{device.name}</span></button>
      {:else}<p>Your connected devices<br />will appear here.</p>{/each}
    </div>
    <div class="sidebar-bottom"><div class="local-note"><Icon name="link" size={18} /><span>Local by design<small>Your hardware. Your data.</small></span></div><div class="sidebar-footer"><span>v{status?.version ?? '0.1.0'}</span><span>API v1 <span class="tiny-dot"></span></span></div></div>
  </aside>

  <div class="main-shell">
    <header class="topbar"><div class="breadcrumb">Workspace <Icon name="chevron" size={13} /><strong>{view === 'overview' ? 'Overview' : 'API console'}</strong></div><div class="topbar-right"><a class="button secondary small" href="/overlay/">Stream overlay ↗</a><span class="mode-tag">{status?.mockMode ? 'MOCK SESSION' : status?.replay ? 'REPLAY SESSION' : 'LOCAL SESSION'}</span><span class="connection-tag" class:offline={!online}><span class="signal-dot"></span>{online ? 'Bridge online' : connection === 'connecting' ? 'Connecting' : 'Reconnecting'}</span></div></header>
    <main>
      <div class="page-heading"><div><div class="eyebrow">BIKEBRIDGE / DEVICE STUDIO</div><h1>{view === 'overview' ? 'Your ride, connected.' : 'A closer look at your API.'}</h1><p>{view === 'overview' ? 'Connect your cycling hardware and see every signal as it happens.' : 'Send a request, inspect a response, and follow the live event stream.'}</p></div><button class="button secondary refresh-button" disabled={!!busy || !online} onclick={() => action('refresh', refresh)}><Icon name="refresh" size={16} />Refresh</button></div>

      {#if notice}<div class="notice" class:error={notice.error} role={notice.error ? 'alert' : 'status'}><Icon name={notice.error ? 'bolt' : 'check'} size={18} /><span>{notice.message}</span><button aria-label="Dismiss notification" onclick={() => notice = null}><Icon name="close" size={16} /></button></div>{/if}
      {#if !online}<div class="notice error" role="status">Waiting for BikeBridge. Start the daemon with <code>cargo run -p bikebridge-cli -- run</code>. This page reconnects automatically.</div>{/if}
      {#if status?.replay}<div class="replay-bar"><span><strong>Trace replay</strong> · {status.replay.speed}× · {status.replay.finished ? 'Finished' : status.replay.playing ? 'Playing' : 'Paused'}</span><div>{#each ['start', 'pause', 'restart'] as replayAction}<button class="button secondary small" disabled={!!busy || !online} onclick={() => action('replay', () => http(`/api/replay/${replayAction}`, 'POST'))}>{replayAction}</button>{/each}</div></div>{/if}

      {#if view === 'overview'}
        <section class="devices-panel panel">
          <div class="panel-header"><div class="section-title"><Icon name="bluetooth" /><h2>Your devices</h2><span class="count">{devices.length}</span></div><div class="panel-actions"><span class="scan-label"><span class="signal-dot" class:muted={!status?.scan.scanning}></span>{status?.mockMode ? 'Simulated devices' : status?.scan.scanning ? 'Scanning nearby' : 'Scan stopped'}</span><button class="button secondary small" disabled={!online || !!busy || !status?.bluetoothEnabled} onclick={() => action('scan', () => http(`/api/scan/${status?.scan.scanning ? 'stop' : 'start'}`, 'POST'))}>{status?.scan.scanning ? 'Stop scan' : 'Start scan'}</button><button class="button primary small" disabled={!online || !!busy || !status?.bluetoothEnabled} onclick={browseBluetooth}><Icon name="bluetooth" size={15} />Browse Bluetooth</button><button class="button secondary small" disabled={!online || !status?.bluetoothEnabled} onclick={() => showNameForm = !showNameForm}><Icon name="plus" size={15} />Find by name</button></div></div>
          {#if status?.scan.lastError}<p class="scan-error">{status.scan.lastError.message}</p>{/if}
          {#if showNearby}
            <div class="nearby-browser">
              <div class="nearby-title"><div><h3>Nearby Bluetooth devices</h3><p>Search Bluetooth names, then add a device to Your devices. Compatibility is checked when you connect.</p></div><button class="text-button" aria-label="Close Bluetooth browser" onclick={() => showNearby = false}><Icon name="close" /></button></div>
              <div class="nearby-tools"><input aria-label="Search Bluetooth names" bind:value={nearbyQuery} placeholder="Search names, e.g. Zwift or Click…" /><label class="checkbox"><input type="checkbox" bind:checked={showUnnamed} />Show unnamed</label><button class="button secondary small" disabled={!online || nearbyLoading} onclick={refreshNearby}>Refresh results</button></div>
              <p class="helper">{visibleNearby.length} results · {status?.scan.scanning ? 'Updating every 3 seconds' : 'Scan stopped'} · Cached results may include devices no longer nearby.</p>
              {#if nearbyError}<p class="scan-error" role="alert">{nearbyError}</p>{/if}
              <div class="nearby-list">
                {#each visibleNearby as device (device.id)}
                  <div class="nearby-row"><div><strong>{device.name ?? 'Unnamed device'}</strong><small>#{device.id.slice(-8)}{device.signalStrength === null ? '' : ` · ${device.signalStrength} dBm`}{device.deviceId ? ' · Added' : ''}</small></div><button class="button secondary small" disabled={!online || !!busy} onclick={() => selectNearby(device)}>{busy === device.id ? 'Adding…' : device.deviceId ? 'View device' : 'Add device'}</button></div>
                {:else}<p class="helper">{nearbyLoading ? 'Loading Bluetooth results…' : nearbyQuery ? 'No matching Bluetooth names. Wake the device and try a shorter search.' : 'No named devices yet. Wake the device, or enable Show unnamed.'}</p>{/each}
              </div>
            </div>
          {/if}
          {#if showNameForm}<form class="name-form" onsubmit={findByName}><label for="device-name">Bluetooth device name<small>Use its full name, such as “Steele’s Bike”. Names are remembered for this daemon session.</small></label><div><input id="device-name" bind:value={deviceName} required maxlength="128" placeholder="Enter a Bluetooth name…" /><button class="button primary" disabled={!!busy || !deviceName.trim() || !online}>{busy === 'name' ? 'Searching…' : 'Find device'}</button></div></form>{/if}
          {#if devices.length > 4}<div class="device-search"><input aria-label="Filter devices" bind:value={query} placeholder="Filter devices by name…" /></div>{/if}
          <div class="device-list">
            {#each filteredDevices as device (device.id)}
              <div class="device-row" class:selected={selectedId === device.id}>
                <button class="device-select" onclick={() => selectedId = device.id} aria-pressed={selectedId === device.id}><span class="device-icon"><Icon name={device.kind === 'bike_controller' ? 'grid' : device.kind === 'heart_rate_monitor' ? 'heart' : 'bike'} size={25} /></span><span class="device-description"><strong>{device.name}</strong><span>{kindName(device.kind)} <span class="separator">/</span> {device.transport === 'mock' ? 'Simulated' : 'Bluetooth LE'}</span></span></button>
                <div class="device-meta">{#if device.signalStrength !== undefined}<span class="rssi" title="Bluetooth signal strength"><Icon name="signal" size={15} />{device.signalStrength} dBm</span>{/if}<span class="device-status" class:connected={device.connected}><span class="signal-dot" class:muted={!device.connected}></span>{device.connected ? 'Connected' : 'Available'}</span><button class="button small" class:secondary={device.connected} class:primary={!device.connected} disabled={!online || !!busy || !!status?.replay} onclick={() => deviceAction(device)}>{busy === device.id ? 'Working…' : device.connected ? 'Disconnect' : 'Connect'}</button></div>
              </div>
            {:else}<div class="empty-devices"><Icon name="bluetooth" size={30} /><strong>{query ? 'No matching devices' : 'Ready when your bike is.'}</strong><p>{query ? 'Try a different name.' : 'Wake your trainer or sensor to discover it. If it stays hidden, use Find by name.'}</p></div>{/each}
          </div>
        </section>

        {#if selected?.kind === 'bike_controller'}
          <section class="panel input-panel">
            <div class="panel-header"><div class="section-title"><Icon name="grid" /><h2>Controller buttons</h2></div><span class="subtle">{selected.connected && online ? 'Listening' : 'Not connected'}</span></div>
            <div class="input-content"><p class="helper">{selected.name} · Press and release a button to see its input here. Button inputs do not change trainer resistance automatically.</p>
              {#if selected.name.startsWith('Zwift Click V2')}<p class="helper">Direct Click V2 support is experimental. If input stops, enable both controllers in the Zwift game, close Zwift, then reconnect here.</p>{/if}
              <div class="input-buttons">{#each controllerInputs as input (input.label)}<div class="input-button" class:held={input.state === 'pressed' && online && selected.connected}><strong>{input.label}</strong><span>{input.state}{input.value === undefined ? '' : ` · ${input.value}`}</span><time>{time(input.at)}</time></div>{:else}<p class="helper">Waiting for a button event…</p>{/each}</div>
            </div>
          </section>
        {:else}
        <div class="telemetry-heading"><div><h2>Live telemetry</h2><span>{selected?.name ?? 'Select a device to get started'}</span></div><span class="live-label" class:stale={!fresh}><span class="signal-dot" class:muted={!fresh}></span>{fresh ? 'LIVE' : selected?.connected ? 'AWAITING DATA' : 'NOT CONNECTED'}</span></div>
        <div class="metrics-grid">
          {#each metrics as metric}<section class="metric-card" class:dim={!fresh}><div class="metric-label"><span>{metric.name}</span><Icon name={metric.icon} size={18} /></div><div class="metric-value">{format(fresh ? latest?.data[metric.key] : undefined, metric.key === 'speedKph' || metric.key === 'cadenceRpm' ? 1 : 0)}<span>{metric.unit}</span></div><span class="metric-foot">{fresh && latest?.data[metric.key] !== undefined ? 'Latest measurement' : 'No current measurement'}</span></section>{/each}
        </div>

        <div class="workspace-grid">
          <section class="panel power-panel"><div class="panel-header"><div class="section-title"><span class="line-key"></span><h2>Power over time</h2></div><span class="subtle">Last 60 seconds</span></div><Chart samples={history[selectedId] ?? []} {now} /><div class="chart-footer"><span>POWER <span class="separator">/</span> WATTS</span><span>{latest ? `Last received ${time(latest.at)}` : 'Waiting for a connected power source'}</span></div></section>
          <section class="panel controls-panel"><div class="panel-header"><div class="section-title"><Icon name="speed" size={19} /><h2>Trainer controls</h2></div></div>
            {#if controllable}
              <div class="control-content"><div class="control-state"><span class="signal-dot" class:muted={ownerId !== selectedId}></span>{ownerId === selectedId ? 'This page has control' : 'Control available on connection'}</div><div class="control-buttons"><button class="button primary small" disabled={!online || !selected?.connected || !!busy || !!status?.replay} onclick={() => control(ownerId === selectedId ? 'trainer.reset' : 'trainer.requestControl')}>{ownerId === selectedId ? 'Release control' : 'Take control'}</button><button class="button secondary small" disabled={!controlsEnabled} onclick={() => control('trainer.start')}>Start</button><button class="button secondary small" disabled={!controlsEnabled} onclick={() => control('trainer.stop')}>Stop</button></div>
                {#if selected?.capabilities.includes('erg_control')}<form class="control-field" onsubmit={e => { e.preventDefault(); void control('trainer.setTargetPower', { watts }); }}><label for="watts">Target power <span>W</span></label><div><input id="watts" type="number" min="0" max="2000" step="1" required bind:value={watts} disabled={!controlsEnabled} /><button class="button secondary small" disabled={!controlsEnabled}>Apply</button></div></form>{/if}
                {#if selected?.capabilities.includes('resistance_control')}<form class="control-field" onsubmit={e => { e.preventDefault(); void control('trainer.setResistance', { resistance: resistance / 100 }); }}><label for="resistance">Resistance <span>%</span></label><div><input id="resistance" type="number" min="0" max="100" required bind:value={resistance} disabled={!controlsEnabled} /><button class="button secondary small" disabled={!controlsEnabled}>Apply</button></div></form>{/if}
                {#if selected?.capabilities.includes('simulation_control')}<form class="control-field" onsubmit={e => { e.preventDefault(); void control('trainer.setSimulation', { gradePercent: grade, windSpeedMps: 0, crr: 0.004, cw: 0.51 }); }}><label for="grade">Simulated grade <span>%</span></label><div><input id="grade" type="number" min="-25" max="25" step="0.1" required bind:value={grade} disabled={!controlsEnabled} /><button class="button secondary small" disabled={!controlsEnabled}>Apply</button></div></form>{/if}
                <p class="helper">Your configured limits apply to every command. Closing this page releases its control.</p>
              </div>
            {:else}<div class="control-empty"><span class="round-icon"><Icon name="speed" size={25} /></span><strong>{selected?.connected ? 'A view into your ride.' : 'Connect to see controls'}</strong><p>{selected?.connected ? 'This device provides measurements. Resistance and ERG controls appear for compatible trainers.' : 'Available controls are verified when your device connects.'}</p></div>{/if}
          </section>
        </div>
        {/if}
        <section class="panel activity-panel"><div class="panel-header"><div class="section-title"><Icon name="terminal" size={18} /><h2>Recent activity</h2><span class="count">{filteredLogs.length}</span></div><button class="text-button" onclick={() => view = 'api'}>Open API console <Icon name="arrow" size={15} /></button></div><div class="compact-log">{#each filteredLogs.slice(0, 5) as item (item.id)}<div><time>{time(item.at)}</time><span class="event-type" class:failure={item.event.type === 'error' || item.event.success === false}>{item.event.type}</span><span class="event-detail">{logDetail(item.event)}</span><span class="direction">{item.event.direction === 'out' ? 'OUT' : 'IN'}</span></div>{:else}<p class="helper">Connection and device events will appear here.</p>{/each}</div></section>
      {:else}
        <section class="panel api-panel"><div class="panel-header"><div class="section-title"><Icon name="code" /><h2>Request builder</h2></div><span class="subtle">Same local API. No extra setup.</span></div><div class="api-presets"><span>Try a request</span><button onclick={() => preset('status')}>Status</button><button onclick={() => preset('devices')}>Devices</button><button onclick={() => preset('name')}>Find by name</button><button onclick={() => preset('connect')}>Connect selected</button><button onclick={() => preset('subscribe')}>Subscribe</button></div><div class="api-request"><select aria-label="Request method" bind:value={apiMethod}><option>GET</option><option>POST</option><option value="WS">WebSocket</option></select>{#if apiMethod !== 'WS'}<input aria-label="API path" bind:value={apiPath} />{:else}<div class="socket-path">/ws <span>Uses this page’s session</span></div>{/if}<button class="button primary" disabled={!online || !!busy} onclick={sendApi}>{busy === 'api' ? 'Sending…' : 'Send request'}<Icon name="arrow" size={16} /></button></div><div class="api-editors">{#if apiMethod !== 'GET'}<label>REQUEST BODY<textarea aria-label="Request body" bind:value={apiBody} spellcheck="false"></textarea></label>{/if}<div class="response-editor"><span>RESPONSE</span><pre aria-live="polite">{apiResult}</pre></div></div></section>
        <section class="panel event-panel"><div class="panel-header"><div class="section-title"><Icon name="terminal" /><h2>Event stream</h2><span class="count">{filteredLogs.length}</span></div><div class="panel-actions"><label class="checkbox"><input type="checkbox" bind:checked={includeTelemetry} />Telemetry</label><button class="text-button" onclick={() => logs = []}>Clear</button><button class="button secondary small" onclick={exportLog}>Export JSON</button></div></div><p class="event-help">Latest 200 messages. Expand any event to inspect its JSON.</p><div class="event-list">{#each filteredLogs as item (item.id)}<details><summary><time>{time(item.at)}</time><span class="event-type" class:failure={item.event.type === 'error' || item.event.success === false}>{item.event.type}</span><span class="event-detail">{logDetail(item.event)}</span><span class="direction">{item.event.direction === 'out' ? 'OUT' : 'IN'}</span></summary><pre>{JSON.stringify(item.event, null, 2)}</pre></details>{:else}<p class="empty-events">Listening for events…</p>{/each}</div></section>
      {/if}
      <footer class="page-footer"><span><span class="signal-dot" class:muted={!online}></span>{status?.mockMode ? 'Mock hardware · simulated measurements' : status?.replay ? 'Trace playback · recorded measurements' : 'Everything runs on this computer'}</span><span>HTTP + WebSocket <span class="separator">/</span> BikeBridge {status?.version ?? ''}</span></footer>
    </main>
  </div>
</div>
