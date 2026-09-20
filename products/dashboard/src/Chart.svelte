<script lang="ts">
  let { samples = [], now }: { samples: { at: number; value?: number }[]; now: number } = $props();
  let visible = $derived(samples.filter(s => s.at >= now - 60000));
  let maximum = $derived(Math.max(100, Math.ceil(Math.max(0, ...visible.map(s => s.value ?? 0)) / 50) * 50));
  let points = $derived.by(() => {
    let path = ''; let drawing = false;
    for (const sample of visible) {
      if (sample.value === undefined) { drawing = false; continue; }
      const x = 44 + Math.max(0, Math.min(1, (sample.at - (now - 60000)) / 60000)) * 710;
      const y = 172 - Math.max(0, sample.value) / maximum * 148;
      path += `${drawing ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)} `; drawing = true;
    }
    return path;
  });
</script>
<svg class="chart" viewBox="0 0 780 205" role="img" aria-label="Power measurements over the last 60 seconds">
  {#each [0, 0.5, 1] as fraction}
    <line x1="44" x2="754" y1={172 - fraction * 148} y2={172 - fraction * 148} stroke="#e6eae5" stroke-dasharray="3 5" />
    <text x="30" y={177 - fraction * 148} text-anchor="end">{maximum * fraction}</text>
  {/each}
  <path d={points} fill="none" stroke="#26785a" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" />
  <text x="44" y="198">−60s</text><text x="394" y="198" text-anchor="middle">−30s</text><text x="754" y="198" text-anchor="end">Now</text>
  {#if !visible.some(s => s.value !== undefined)}<text x="399" y="101" text-anchor="middle" class="empty-chart">Waiting for power measurements</text>{/if}
</svg>
<style>
  .chart { width:100%; display:block; min-height:160px; }
  text { font:11px ui-monospace,monospace; fill:#7d8983; }
  .empty-chart { font:13px system-ui,sans-serif; fill:#819086; }
</style>
