<script lang="ts">
  let { name, size = 16 }: { name: string; size?: number } = $props();
  const paths: Record<string, string> = {
    pin: 'M16 3l5 5-4 1-3 4 1 4-2 2-4-4-6 6 6-6-4-4 2-2 4 1 4-3z',
    settings: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8 M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1z',
    close: 'M6 6l12 12M6 18L18 6', minimize: 'M5 12h14',
    compact: 'M8 3v5H3m18 0h-5V3M3 16h5v5m8 0v-5h5',
    expand: 'M8 3H3v5m13-5h5v5M3 16v5h5m8 0h5v-5',
    play: 'M8 5l11 7-11 7z', pause: 'M8 5v14M16 5v14',
    reset: 'M3 10a9 9 0 1 1 1 7M3 4v6h6',
    bolt: 'M13 2L4 14h7l-1 8 10-13h-8z',
    cadence: 'M12 3a9 9 0 1 0 9 9M12 8v4l6-6M18 3h3v3',
    speed: 'M4 18a9 9 0 1 1 16 0M12 14l5-6M7 18h10',
    heart: 'M20 5a5 5 0 0 0-8 1 5 5 0 0 0-8-1c-5 5 8 15 8 15S25 10 20 5z',
  };
</script>
<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name] ?? paths.bolt} /></svg>
