<script lang="ts">
  let { name, size = '1.25rem' }: { name: string; size?: string } = $props();
  const paths: Record<string, string> = {
    menu: 'M4 6h16M4 12h16M4 18h16',
    monitor: 'M3 3h18v13H3zM12 16v5M8 21h8',
    globe: 'M3 12h18M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18M12 3c-5 5-5 13 0 18M12 3c5 5 5 13 0 18',
    cloud: 'M6 18h12a4 4 0 0 0 0-8 6 6 0 0 0-11-2 5 5 0 0 0-1 10',
    installed: 'M8 12l3 3 5-6M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0',
    search: 'm21 21-4.6-4.6M19 10.8a8.2 8.2 0 1 1-16.4 0 8.2 8.2 0 0 1 16.4 0',
    refresh: 'M20 7v5h-5M4 17v-5h5M6.1 6.1A8 8 0 0 1 19.7 10M4.3 14a8 8 0 0 0 13.6 3.9',
    grid: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',
    play: 'm9 5 11 7-11 7z',
    arrow: 'M5 12h14m-5-5 5 5-5 5',
    close: 'm6 6 12 12M18 6 6 18',
    minimize: 'M5 12h14',
    maximize: 'M5 5h14v14H5z',
    fullscreen: 'M8 3H3v5M16 3h5v5M3 16v5h5M21 16v5h-5',
    more: 'M5 12h.01M12 12h.01M19 12h.01',
    external: 'M14 3h7v7m0-7L10 14M10 3H3v18h18v-7',
    trash: 'M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7',
    download: 'M12 3v12m-5-5 5 5 5-5M4 17v4h16v-4',
    check: 'm5 12 4 4L20 5',
    drive: 'M5 4h14l3 13v4H2v-4zM2 17h20M6 19h.01',
    wine: 'M7 3h10l1 7a6 6 0 0 1-12 0zM12 16v5M8 21h8',
    clock: 'M12 7v5l3 2M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0'
  };
</script>
{#if name === 'windows'}
  <svg style:width={size} style:height={size} viewBox="0 0 24 24" fill="#00a4ef" aria-hidden="true"><path d="M2 4.7 10.5 3.5v8H2zm10-1.5L22 2v9.5h-10zM2 13h8.5v8L2 19.8zm10 0h10v9l-10-1.3z" /></svg>
{:else if name === 'xbox'}
  <svg style:width={size} style:height={size} viewBox="0 0 24 24" fill="#32b94a" aria-hidden="true"><circle cx="12" cy="12" r="10" opacity=".2"/><path d="M4.6 5.2C7 4 10 6.3 12 8.2c2-1.9 5-4.2 7.4-3C17.7 7.2 15.9 9.3 14 12c2.9 3.7 4.8 7.4 4 8C17 18.5 14 15.8 12 14c-2 1.8-5 4.5-6 6-.8-.6 1.1-4.3 4-8C8.1 9.3 6.3 7.2 4.6 5.2"/></svg>
{:else if ['steam', 'epic', 'ea', 'ubisoft', 'linux'].includes(name)}
  <img style:width={size} style:height={size} src={`/icons/${name === 'epic' ? 'epicgames' : name}.svg`} alt="" aria-hidden="true" />
{:else}
  <svg style:width={size} style:height={size} viewBox="0 0 24 24" fill={name === 'play' ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[name] ?? paths.grid} /></svg>
{/if}
<style>img { object-fit: contain; } svg, img { flex-shrink: 0; display: inline-block; vertical-align: middle; }</style>
