<script lang="ts">
  let { name, size = 20 }: { name: string; size?: number } = $props();
  const paths: Record<string, string> = {
    bridge: 'M3 17V9m18 8V9M3 12c5 0 4-7 9-7s4 7 9 7M3 17h18M8 10v7m8-7v7M12 5v12',
    grid: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',
    terminal: 'm4 6 5 6-5 6m9 0h7',
    bluetooth: 'm7 7 10 10-5 4V3l5 4L7 17',
    arrow: 'M5 12h14m-5-5 5 5-5 5',
    refresh: 'M20 7v5h-5M4 17v-5h5M6 6a8 8 0 0 1 13 3M5 15a8 8 0 0 0 13 3',
    bolt: 'm13 2-9 12h7l-1 8 10-13h-7z',
    cadence: 'M12 3a9 9 0 1 0 9 9M12 8v4l3 2m-3-11v4h4',
    speed: 'M4 19a10 10 0 1 1 16 0M12 12l5-5M5 13h1m12 0h1M8 6l1 1m3-4v2',
    heart: 'M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z',
    bike: 'M7 17a4 4 0 1 1-8 0 4 4 0 0 1 8 0Zm17 0a4 4 0 1 1-8 0 4 4 0 0 1 8 0ZM3 17l5-9 6 9H3m11 0 4-12h-4M6 5h5m6 3 3 9',
    plus: 'M12 5v14M5 12h14',
    chevron: 'm9 5 7 7-7 7',
    signal: 'M4 20v-4m5 4v-8m5 8V8m5 12V4',
    play: 'm7 4 14 8-14 8z',
    stop: 'M5 5h14v14H5z',
    code: 'm8 6-6 6 6 6m8-12 6 6-6 6m-3-14-2 16',
    check: 'm5 12 4 4L19 6',
    close: 'm6 6 12 12M6 18 18 6',
    link: 'm9 15 6-6M7 17l-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m4-1 1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0',
  };
</script>
<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name] ?? paths.bike} /></svg>
