<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke, convertFileSrc } from '@tauri-apps/api/core';
  import Icon from './Icon.svelte';
  import { installedHere, storeNames, type Game, type OS } from './library';
  let { game, host, action, onArtwork, onTitle }: { game: Game; host: OS; action: (game: Game, kind: string) => void; onArtwork: (url: string) => void; onTitle: (title: string) => void } = $props();
  let element: HTMLElement;
  let cover = $state('');
  let pending = $state(true);
  let loaded = $state(false);
  let menu = $state(false);
  let nearby = $state(false);
  let hovering = false;
  let hoverTimer: ReturnType<typeof setTimeout>;
  function enter() {
    hovering = true;
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => { if (hovering && cover) onArtwork(cover); }, 600);
  }
  function leave() { if (hovering) onArtwork(''); hovering = false; clearTimeout(hoverTimer); }
  const local = $derived(installedHere(game, host));
  const other = $derived([...new Set(game.installations.filter(i => i.os !== host && i.os !== 'unknown' && i.state !== 'files_missing').map(i => i.os))]);
  const label = $derived(local ? 'Installed' : other.length ? `On ${other.map(os => os === 'windows' ? 'Windows' : 'Linux').join(' & ')}` : 'Not installed');
  async function artwork() {
    try {
      const result = await invoke<{ path: string; title: string | null }>('game_artwork', { key: game.key });
      cover = convertFileSrc(result.path);
      if (hovering) enter();
      if (result.title && game.title === game.id) onTitle(result.title);
    } catch { /* Covers are optional; the title remains usable. */ }
    finally { pending = false; }
  }
  onMount(() => {
    let requested = false;
    const observer = new IntersectionObserver(entries => {
      nearby = entries.some(e => e.isIntersecting);
      if (nearby && !requested) { requested = true; void artwork(); }
    }, { rootMargin: '25%' });
    observer.observe(element);
    return () => { observer.disconnect(); leave(); };
  });
</script>

<article aria-label={`${game.title}, ${label}`} class:away={!local} bind:this={element} onmouseenter={enter} onmouseleave={leave}>
  <div class="art">
    {#if pending || (cover && !loaded)}<div class="skeleton" class:running={nearby}></div>{/if}
    {#if cover}<img class:loaded src={cover} alt="" loading="lazy" decoding="async" onload={() => { loaded = true; }} onerror={() => { cover = ''; loaded = false; }} />{/if}
    {#if !cover && !pending}<div class="fallback"><span>{game.title.split(/\s+/).slice(0, 2).map(w => w[0]).join('')}</span></div>{/if}
    <div class="shade"></div>
    <span class="client" title={`${storeNames[game.store]} · ${game.client}`}><Icon name={game.store} size="1.125rem"/>{#if ![storeNames[game.store], 'Epic', 'steam', 'epic', 'ea', 'ubisoft', 'xbox'].includes(game.client)}<span class="runner">{game.client.includes('Heroic') ? 'H' : game.client.includes('Lutris') ? 'L' : game.client.includes('Bottles') ? 'B' : game.client.includes('Nexus') ? 'N' : game.client.includes('Ferestre') ? 'F' : 'P'}</span>{/if}</span>
    <button class="more" aria-label={`Options for ${game.title}`} aria-expanded={menu} onclick={() => menu = !menu}><Icon name="more" size="1.3125rem"/></button>
    {#if menu}<div class="menu" role="group" aria-label="Game actions">
      <button disabled={!game.launch || !local} onclick={() => { menu = false; action(game, 'launch'); }}><Icon name="play" size="1rem"/>Launch</button>
      <button disabled={!game.store_page} onclick={() => { menu = false; action(game, 'store'); }}><Icon name="external" size="1rem"/>View in store</button>
      <button class="danger" disabled={!game.uninstall || !local} onclick={() => { menu = false; action(game, 'uninstall'); }}><Icon name="trash" size="1rem"/>Uninstall…</button>
    </div>{/if}
    {#if !local && other.length}<div class="os" title={label}>{#each other as os}<Icon name={os} size="2.0rem"/>{/each}</div>{/if}
    <div class="bottom">
      <span class="state" title={label}>{#if local}<span class="dot"></span>{/if}</span>
      <button class="quick" class:play={local} disabled={local ? !game.launch : !game.store_page} aria-label={`${local ? 'Launch' : 'Open store for'} ${game.title}`} onclick={() => action(game, local ? 'launch' : 'store')}><Icon name={local ? 'play' : 'external'} size="1.125rem"/></button>
    </div>
  </div>
  <div class="caption"><h3 title={game.title}>{game.title}</h3></div>
</article>

<style>
  article { min-width: 0; position: relative; transition: transform .2s ease; }
  article:hover { transform: translateY(-0.25rem); }
  .art { aspect-ratio: 3/4; position: relative; overflow: hidden; border-radius: 0.875rem; background: #202424; box-shadow: 0 0.5625rem 1.5rem #0002; }
  .art > img { position: absolute; width: 100%; height: 100%; object-fit: cover; opacity: 0; transition: opacity .25s; }
  .art > img.loaded { opacity: 1; }
  .away .fallback { filter: grayscale(1) brightness(.75); } .away .art > img { filter: grayscale(1) brightness(.55); } .away .caption { opacity: .65; }
  .shade { position: absolute; inset: 0; background: linear-gradient(180deg,#080c0a45,transparent 30%,#080c0a0d 50%,#080c0ae6); pointer-events: none; }
  .client,.quick { width: 2.0625rem; height: 2.0625rem; border-radius: 50%; display: grid; place-items: center; border: 0.0625rem solid #ffffff1c; }
  .client { position: absolute; top: 0.6875rem; left: 0.6875rem; background: #121715cf; }
  .runner { position: absolute; right: -.1875rem; bottom: -.125rem; display: grid; place-items: center; width: .875rem; height: .875rem; border-radius: 50%; background: #121715; font: 700 .5625rem system-ui; color: #c5d3ce; }
  button { color: #f0f3f1; border: 0; cursor: pointer; } button:disabled { opacity: .3; cursor: default; }
  .more { position: absolute; right: 0.5rem; top: 0.5625rem; background: #121715c0; border-radius: 50%; width: 2.25rem; height: 2.25rem; padding: 0; display: grid; place-items: center; opacity: 0; transition: opacity .15s; } article:hover .more, .more:focus-visible { opacity: 1; }
  .menu { position: absolute; z-index: 3; top: 2.75rem; right: 0.5rem; width: min(11.25rem, calc(100% - 1rem)); padding: 0.3125rem; border: 0.0625rem solid #ffffff20; border-radius: 0.625rem; background: #202724f5; box-shadow: 0 0.625rem 2rem #0007; }
  .menu button { background: transparent; width: 100%; padding: 0.625rem; display: flex; align-items: center; gap: 0.5625rem; text-align: left; border-radius: 0.375rem; font: 0.875rem system-ui; } .menu button:hover:enabled { background: #ffffff12; } .menu .danger { color: #ffb5c1; }
  .bottom { position: absolute; left: 0.8125rem; right: 0.6875rem; bottom: 0.75rem; display: flex; justify-content: space-between; align-items: center; gap: 0.25rem; }
  .state { font: 0.875rem system-ui; color: #e6e4ef; display: flex; align-items: center; gap: 0.3125rem; } .dot { width: 0.3125rem; height: 0.3125rem; border-radius: 50%; background: #91e5ba; box-shadow: 0 0 0.5rem #91e5ba60; }
  .quick { background: #ffffff18; } .quick.play { background: #dce8e2; color: #1b2922; } .quick:hover:enabled { background: #edf3ef; color: #1b2922; }
  .os { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; gap: 0.75rem; color: #ffffffc0; pointer-events: none; }
  .caption { padding: 0.75rem 0.125rem 0.25rem; } h3 { font-size: 1rem; font-weight: 600; line-height: 1.4; margin: 0 0 0.25rem; overflow: hidden; display: -webkit-box; line-clamp: 2; -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
  .fallback { position: absolute; inset: 0; display: grid; place-items: center; background: radial-gradient(ellipse at 10% 20%,#32413b80,transparent 70%),linear-gradient(135deg,#242b29,#161b19); } .fallback > span { font-size: 2.8125rem; font-weight: 800; color: #ffffff20; letter-spacing: -0.25rem; }
  .skeleton { position: absolute; inset: 0; background: linear-gradient(105deg,#222927 35%,#303a35 50%,#222927 65%); background-size: 300% 100%; animation: shimmer 1.7s ease-in-out 3 paused; }
  .skeleton.running { animation-play-state: running; }
  @keyframes shimmer { to { background-position: -150% 0; } } @media(prefers-reduced-motion:reduce) { * { animation: none !important; transition: none !important; } }
</style>
