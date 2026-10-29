<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen } from '@tauri-apps/api/event';
  import Icon from '$lib/Icon.svelte';
  import Backdrop from '$lib/Backdrop.svelte';
  import GameCard from '$lib/GameCard.svelte';
  import { emptyFilters, gameForOS, gameView, matchesFilters, storeNames, stores, type Filters, type Game, type OS, type Store } from '$lib/library';

  type Report = {
    host_os: OS; catalog: Game[]; scanned_at: number; warnings: string[];
    volumes: { id: string; filesystem: string; mountpoints: string[]; os: OS; note: string }[];
    epic: { legendary_available: boolean; sources: { path: string; kind: string; local: boolean }[] };
  };
  let zoom = $state(100);
  function setZoom(value: number) {
    zoom = Math.max(75, Math.min(200, Math.round(value)));
    document.documentElement.style.fontSize = `${zoom}%`;
    try { localStorage.setItem('raesir.zoom', String(zoom)); } catch { /* Zoom still works when storage is unavailable. */ }
  }
  let report = $state.raw<Report | null>(null);
  let starting = $state(true);
  let refreshing = $state(false);
  let ownership = $state(false);
  let closing = $state(false);
  let scrolled = $state(false);
  let search = $state('');
  let filters = $state<Filters>({ ...emptyFilters });
  let toast = $state('');
  let error = $state(false);
  let hero = $state('');
  let visible = $state(96);
  let pendingAction = $state('');
  let confirm = $state<Game | null>(null);
  let dialog = $state<HTMLDialogElement>();
  let sentinel: HTMLDivElement;
  let searchInput: HTMLInputElement;
  let toastTimer: ReturnType<typeof setTimeout>;
  const host = $derived(report?.host_os ?? 'unknown');
  const all = $derived(report?.catalog ?? []);
  const games = $derived(all.filter(g => matchesFilters(gameView(g), search, filters, host)).map(g => gameForOS(g, filters, host)));
  const selectedStores = $derived(stores.filter(s => filters[s]));
  const availableStores = $derived(stores.filter(s => all.some(g => g.store === s)));
  const filterKey = $derived(search + Object.values(filters).join(','));
  const mounts = $derived(report?.volumes.filter(v => !v.mountpoints.length && ['ntfs', 'ext4', 'btrfs', 'linux'].includes(v.filesystem.toLowerCase())) ?? []);
  $effect(() => { filterKey; visible = 96; });
  $effect(() => { if (report) for (const store of stores) { if (filters[store] && !availableStores.includes(store)) filters[store] = false; } });
  $effect(() => { if (confirm && dialog && !dialog.open) dialog.showModal(); });

  function notify(message: string, failed = false) {
    toast = message; error = failed; clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { toast = ''; }, failed ? 14000 : 6500);
  }
  async function refresh(command = 'scan_library', args: Record<string, unknown> = {}) {
    if (refreshing || closing) return;
    refreshing = true;
    try { report = await invoke<Report>(command, args); }
    catch (e) { notify(String(e), true); }
    finally { starting = false; refreshing = false; }
  }
  async function steamOwnership() {
    if (ownership || closing) return;
    ownership = true;
    try {
      const session = await invoke<{ supported: boolean; managed_root: string | null; clients: { root: string }[] }>('steam_status');
      const root = session.managed_root ?? session.clients[0]?.root;
      if (session.supported && root && !closing) report = await invoke<Report>('import_client_library', { root });
    } catch { /* Cached account data stays visible while Steam signs in. */ }
    finally { ownership = false; }
  }
  async function rescan() {
    if (refreshing || ownership || closing) return;
    await refresh();
    if (closing) return;
    await steamOwnership();
    const profiles = report?.epic.sources.filter(s => s.kind === 'legendary' && s.local) ?? [];
    if (!report?.epic.legendary_available) return;
    for (const profile of profiles) {
      if (closing) return;
      await refresh('refresh_epic', { profile: profile.path });
    }
  }
  function status(kind: 'all' | 'installed' | 'notInstalled') {
    filters.installed = kind === 'installed'; filters.notInstalled = kind === 'notInstalled';
  }
  function location(kind: 'all' | 'thisOS' | 'otherOS') {
    filters.thisOS = kind === 'thisOS'; filters.otherOS = kind === 'otherOS';
  }
  function selectStore(store?: Store) {
    if (!store) for (const s of stores) filters[s] = false;
    else filters[store] = !filters[store];
  }
  async function action(game: Game, kind: string) {
    if (kind === 'uninstall') { confirm = game; return; }
    await dispatch(game, kind);
  }
  async function dispatch(game: Game, kind: string) {
    if (pendingAction || closing) return;
    pendingAction = game.key;
    try { notify(await invoke<string>('game_action', { key: game.key, kind })); }
    catch (e) { notify(String(e), true); }
    finally { pendingAction = ''; }
    if (kind === 'uninstall') setTimeout(() => { void refresh(); }, 3500);
  }
  async function fullscreen() {
    const window = getCurrentWindow(); await window.setFullscreen(!await window.isFullscreen());
  }
  function keys(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && !event.altKey) {
      if (['+', '=', '-', '_', '0'].includes(event.key) || ['NumpadAdd', 'NumpadSubtract', 'Numpad0'].includes(event.code)) {
        event.preventDefault();
        setZoom(event.key === '0' || event.code === 'Numpad0' ? 100 : zoom + (['-', '_'].includes(event.key) || event.code === 'NumpadSubtract' ? -10 : 10));
        return;
      }
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') { event.preventDefault(); searchInput.focus(); }
    if (event.key === 'F11') { event.preventDefault(); void fullscreen(); }
    if (event.key === 'Escape') confirm = null;
  }
  onMount(() => {
    try {
      const saved = Number(localStorage.getItem('raesir.zoom'));
      if (Number.isFinite(saved) && saved >= 75 && saved <= 200) setZoom(saved);
    } catch { /* Use the default size if storage is unavailable. */ }
    const window = getCurrentWindow();
    void window.show();
    let disposed = false;
    const listeners = [
      listen('app-shutting-down', () => { closing = true; toast = ''; }),
      listen<string>('app-shutdown-error', e => { closing = false; notify(e.payload, true); })
    ];
    for (const listener of listeners) void listener.then(stop => { if (disposed) stop(); });
    const observer = new IntersectionObserver(entries => {
      if (entries.some(e => e.isIntersecting) && games.length > visible) visible += 96;
    }, { rootMargin: '50%' });
    observer.observe(sentinel);
    void (async () => {
      try { report = await invoke<Report | null>('load_library'); } catch { /* First run has no cache. */ }
      if (report) starting = false;
      await refresh(); if (!disposed && !closing) void steamOwnership();
    })();
    const focus = () => { if (report && Date.now() / 1000 - report.scanned_at > 90) void refresh(); };
    const scroll = () => { scrolled = globalThis.scrollY > 0; };
    scroll();
    globalThis.addEventListener('scroll', scroll, { passive: true });
    globalThis.addEventListener('focus', focus);
    globalThis.addEventListener('keydown', keys);
    return () => {
      disposed = true; observer.disconnect(); clearTimeout(toastTimer);
      globalThis.removeEventListener('focus', focus); globalThis.removeEventListener('keydown', keys);
      globalThis.removeEventListener('scroll', scroll);
      for (const listener of listeners) void listener.then(stop => stop());
    };
  });
</script>

<svelte:head><title>Raesir</title></svelte:head>
<Backdrop image={hero}/>
<header class="titlebar" class:scrolled data-tauri-drag-region>
  <nav class="platforms" aria-label="Platforms">
    <button class:active={!selectedStores.length} aria-label="All platforms" title="All platforms" aria-pressed={!selectedStores.length} onclick={() => selectStore()}><Icon name="grid"/></button>
    {#each availableStores as store}<button class:active={filters[store]} aria-label={storeNames[store]} title={storeNames[store]} aria-pressed={filters[store]} onclick={() => selectStore(store)}><Icon name={store}/></button>{/each}
  </nav>
  <label class="search"><Icon name="search" size="1.125rem"/><input bind:this={searchInput} type="search" bind:value={search} placeholder="Search games" aria-label="Search games"/></label>
  <div class="header-right">
  <div class="filters">
    <div class="segmented" role="group" aria-label="Installation status">
      <button class:active={!filters.installed && !filters.notInstalled} aria-pressed={!filters.installed && !filters.notInstalled} aria-label="All games" title="All games" onclick={() => status('all')}><Icon name="grid"/></button>
      <button class:active={filters.installed} aria-pressed={filters.installed} aria-label="Installed" title="Installed" onclick={() => status('installed')}><Icon name="installed"/></button>
      <button class:active={filters.notInstalled} aria-pressed={filters.notInstalled} aria-label="Not installed" title="Not installed" onclick={() => status('notInstalled')}><Icon name="cloud"/></button>
    </div>
    <div class="segmented" role="group" aria-label="Operating system">
      <button class:active={!filters.thisOS && !filters.otherOS} aria-pressed={!filters.thisOS && !filters.otherOS} aria-label="All operating systems" title="All operating systems" onclick={() => location('all')}><Icon name="globe"/></button>
      <button class:active={filters.thisOS} aria-pressed={filters.thisOS} aria-label="This OS" title="This OS" onclick={() => location('thisOS')}><Icon name="monitor"/></button>
      <button class:active={filters.otherOS} aria-pressed={filters.otherOS} aria-label="Other OS" title="Other OS" onclick={() => location('otherOS')}><Icon name="drive"/></button>
    </div>
  </div>

  <div class="app-actions">
  <button class="sync" aria-label="Sync library" title={ownership ? 'Syncing Steam' : refreshing ? 'Syncing library' : 'Sync library'} aria-busy={refreshing || ownership} disabled={refreshing || ownership || closing} onclick={rescan}><span class:spin={refreshing || ownership}><Icon name="refresh" size="1rem"/></span></button>
  <details class="refresh-menu"><summary title="Options" aria-label="Options"><Icon name="menu"/></summary><div>
    <div class="zoom-controls" role="group" aria-label="Interface zoom"><button onclick={() => setZoom(zoom - 10)} disabled={zoom <= 75} aria-label="Zoom out" title="Ctrl −">−</button><button onclick={() => setZoom(100)} aria-label="Reset zoom" title="Ctrl 0">{zoom}%</button><button onclick={() => setZoom(zoom + 10)} disabled={zoom >= 200} aria-label="Zoom in" title="Ctrl +">+</button></div>
  </div></details>
  </div>
  <div class="window-actions"><button title="Minimize" aria-label="Minimize" onclick={() => getCurrentWindow().minimize()}><Icon name="minimize" size="1rem"/></button><button title="Maximize" aria-label="Toggle maximize" onclick={() => getCurrentWindow().toggleMaximize()}><Icon name="maximize" size="1rem"/></button><button class="close" title="Close" aria-label="Close Raesir" onclick={() => getCurrentWindow().close()}><Icon name="close" size="1.125rem"/></button></div>
  </div>
</header>

<main>
  {#if mounts.length}<div class="mounts">{#each mounts as volume}<div class="mount"><span class="mount-icon"><Icon name={volume.os === 'linux' || (volume.os === 'unknown' && volume.filesystem !== 'ntfs') ? 'linux' : 'windows'} size="1.0625rem"/></span><span class="mount-copy"><span>{volume.os === 'unknown' ? 'Possible ' : ''}{volume.os === 'windows' || volume.filesystem === 'ntfs' ? 'Windows' : 'Linux'} installation detected</span>{#if host === 'windows'}<small>{volume.note}</small>{/if}</span><button disabled={refreshing || host !== 'linux'} onclick={() => refresh('mount_volume', { id: volume.id })}>{host === 'linux' ? 'Mount & scan' : 'Filesystem support needed'}<Icon name="arrow" size="1rem"/></button></div>{/each}</div>{/if}
  {#if host === 'linux' && selectedStores.includes('xbox')}<details class="runtime-help"><summary>Xbox on Linux</summary><p>Xbox on Linux uses <button onclick={() => invoke('open_support', { project: 'ferestre' }).catch(e => notify(String(e), true))}>Ferestre</button> or <button onclick={() => invoke('open_support', { project: 'nexus' }).catch(e => notify(String(e), true))}>Xodus Nexus</button>. Set up a supported GDK / MSIXVC game there, then refresh. UWP packages remain Windows-only.</p></details>{/if}
  {#if report?.warnings.length}<details class="scan-warnings"><summary>{report.warnings.length} scan notice{report.warnings.length === 1 ? '' : 's'}</summary>{#each report.warnings as warning}<p>{warning}</p>{/each}</details>{/if}
  <section class="collection" aria-label={`Game collection, ${games.length} games`}>
    <div class="grid" aria-busy={starting}>
      {#if starting}{#each Array(12) as _, index}<div class="skeleton-card" style:animation-delay={`${index * 35}ms`}><div></div><span></span><small></small></div>{/each}
      {:else}{#each games.slice(0, visible) as game (game.key)}<GameCard {game} {host} {action} onArtwork={(url) => { hero = url; }} onTitle={(title) => { if (report) report = { ...report, catalog: report.catalog.map(g => g.key === game.key ? { ...g, title } : g) }; }}/>{/each}{/if}
    </div>
    {#if !starting && !games.length}<div class="empty"><Icon name="grid" size="2.25rem"/><h2>{search || selectedStores.length || filters.installed || filters.notInstalled || filters.thisOS || filters.otherOS ? 'No matching games' : 'No games yet'}</h2><p>{all.length ? 'Try a different filter or search.' : 'Sign in to your game clients, then refresh your library.'}</p><button onclick={() => { search = ''; filters = { ...emptyFilters }; if (!all.length) void rescan(); }}>{all.length ? 'Clear filters' : 'Refresh library'}</button></div>{/if}
    <div class="sentinel" bind:this={sentinel}></div>
  </section>
</main>

{#if toast}<div class="toast" class:error role={error ? 'alert' : 'status'}><Icon name={error ? 'close' : 'check'} size="1.0625rem"/><span>{toast}</span><button aria-label="Dismiss notification" onclick={() => toast = ''}><Icon name="close" size="1rem"/></button></div>{/if}
{#if confirm}<dialog class="modal" bind:this={dialog} oncancel={() => confirm = null} aria-labelledby="confirm-title"><span class="modal-icon"><Icon name="trash" size="1.5625rem"/></span><h2 id="confirm-title">Uninstall {confirm.title}?</h2><p>{confirm.uninstall?.note ?? `${confirm.uninstall?.client ?? confirm.client} will handle removing this installation.`}</p><div><button onclick={() => confirm = null}>Keep game</button><button class="destructive" onclick={() => { const game = confirm; confirm = null; if (game) void dispatch(game, 'uninstall'); }}>Continue to uninstall</button></div></dialog>{/if}
{#if closing}<div class="modal-backdrop"><section class="modal closing" role="status"><span class="spin"><Icon name="refresh" size="1.6875rem"/></span><h2>Finishing up</h2><p>Closing managed Steam and releasing mounted partitions.</p></section></div>{/if}

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) { margin: 0; isolation: isolate; background: #101313; color: #edf1ee; font: 1rem/1.5 Inter, ui-sans-serif, system-ui, sans-serif; }
  :global(button), :global(input) { font: inherit; }
  :global(button) { cursor: pointer; -webkit-app-region: no-drag; }
  :global(button:disabled) { cursor: default; opacity: .35; }
  :global(:focus) { outline: none; }
  :global(::selection) { background: #728c7c66; }
  .titlebar { position: fixed; z-index: 20; inset: 0 0 auto; min-height: 4.5rem; display: grid; grid-template-columns: minmax(0,1fr) clamp(8rem,26vw,28rem) minmax(0,1fr); align-items: center; padding: .75rem 1rem; gap: 1rem; user-select: none; }
  .header-right { display: flex; align-items: center; justify-content: flex-end; gap: .75rem; min-width: 0; }
  .platforms,.filters,.segmented,.window-actions,.app-actions { display: flex; align-items: center; flex-shrink: 0; gap: .125rem; }
  .platforms { width: max-content; max-width: 100%; padding: .1875rem; min-width: 0; overflow-x: auto; scrollbar-width: thin; }
  .platforms button { flex-shrink: 0; }
  .filters { gap: .75rem; min-width: 0; flex-shrink: 1; overflow-x: auto; scrollbar-width: thin; }
  .platforms,.segmented,.search,.window-actions,.app-actions { background: #111815b3; border-radius: 999rem; }
  .scrolled :is(.platforms,.segmented,.search,.window-actions,.app-actions) { -webkit-backdrop-filter: blur(.75rem); backdrop-filter: blur(.75rem); }
  .segmented,.window-actions,.app-actions { padding: .1875rem; }
  .platforms button,.segmented button,.window-actions button,.app-actions > button { width: 2.25rem; height: 2.25rem; display: grid; place-items: center; padding: .375rem; border: 0; border-radius: 50%; background: transparent; color: #98a69e; }
  .platforms button:hover,.segmented button:hover,.window-actions button:hover,.app-actions > button:hover:enabled { background: #ffffff08; color: #e9efeb; }
  .platforms button.active,.segmented button.active { background: #ffffff12; color: #f2f5f3; }
  .search { display: flex; align-items: center; min-width: 0; width: 100%; gap: .625rem; padding: .5rem .75rem; border: 0; color: #a2b0a8; }
  .search input { width: 100%; color: #edf1ee; background: transparent; border: 0; outline: none; min-width: 0; }
  .search input::placeholder { color: #8a9990; }
  .window-actions button { width: 2.25rem; color: #a5b1aa; }
  .window-actions .close:hover { background: #85434c; color: white; }
  .sync { flex-shrink: 0; }
  .sync span { display: inline-flex; } .sync:disabled { opacity: 1; }
  .refresh-menu { position: relative; flex-shrink: 0; }
  .refresh-menu summary { color: #b1beb6; cursor: pointer; list-style: none; display: grid; place-items: center; width: 2.25rem; height: 2.25rem; border-radius: 50%; }
  .refresh-menu summary:hover { background: #ffffff08; }
  .refresh-menu > div { position: absolute; top: 2.75rem; right: 0; z-index: 10; padding: .3125rem; width: 15rem; background: #202823; border: .0625rem solid #ffffff12; border-radius: .5625rem; box-shadow: 0 .625rem 2rem #0005; }
  .refresh-menu button { display: block; width: 100%; padding: .625rem; border: 0; border-radius: .3125rem; text-align: left; background: none; color: #d1ddd5; font-size: .9375rem; }
  .refresh-menu button:hover:enabled { background: #ffffff08; }
  .zoom-controls { display: flex; }
  .zoom-controls button { text-align: center; }
  main { padding: 6rem 2rem 2rem; min-height: 100vh; }
  .mounts { display: flex; flex-wrap: wrap; gap: .5rem; margin-bottom: 1.5rem; }
  .mount { display: flex; align-items: center; gap: .5625rem; padding: .625rem .75rem; background: #a9996c09; border: .0625rem solid #c9a96612; border-radius: .5625rem; font-size: .875rem; color: #c6bda3; flex: 1; min-width: min(100%,22rem); }
  .mount button { margin-left: auto; display: flex; align-items: center; gap: .375rem; border: 0; background: none; color: #d0c299; font-size: .875rem; padding: .0625rem; white-space: nowrap; }
  .runtime-help,.scan-warnings { font-size: .875rem; color: #aabbb1; margin: 0 0 1.375rem; }
  .runtime-help button { color: #c1d8ca; background: none; border: 0; padding: 0; text-decoration: underline; }
  .runtime-help summary,.scan-warnings summary { cursor: pointer; }
  .scan-warnings p { overflow-wrap: anywhere; }
  .mount-copy { display: grid; gap: .1875rem; }
  .mount-copy small { color: #b2a992; font-size: .8125rem; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill,minmax(11rem,1fr)); gap: 1.5rem 1.25rem; overflow: visible; }
  .sentinel { height: .0625rem; }
  .skeleton-card > div { aspect-ratio: 3/4; background: #ffffff05; border-radius: .875rem; }
  .skeleton-card > span,.skeleton-card > small { display: block; height: 1rem; border-radius: .1875rem; background: #ffffff07; margin: .875rem 0 .4375rem; width: 70%; }
  .skeleton-card > small { width: 35%; height: .5rem; margin-top: 0; }
  .skeleton-card { animation: pulse 1.7s infinite ease-in-out; }
  .empty { text-align: center; width: min(30rem,100%); margin: 3rem auto 0; padding: 3rem 2rem; border-radius: 2rem; background: #111815e6; color: #b4c6bc; }
  .empty h2 { font-size: 1.5rem; color: #d1e1d7; font-weight: 500; margin-top: 1.5rem; }
  .empty p { font-size: 1rem; }
  .empty button { margin-top: 1.25rem; padding: .625rem 1rem; border: 0; border-radius: 999rem; background: #ffffff12; color: #d1e1d7; }
  .toast { position: fixed; z-index: 60; left: 50%; bottom: 1.5rem; transform: translateX(-50%); background: #111815e6; border: .0625rem solid #ffffff12; box-shadow: 0 .75rem 2rem #0005; border-radius: 1.5rem; padding: .8125rem 1rem; display: flex; align-items: center; gap: .625rem; color: #d2e2d8; max-width: min(40rem,90vw); font-size: .9375rem; }
  .toast.error { border-color: #e58a9a45; color: #eeb5bf; }
  .toast button { border: 0; color: inherit; background: none; padding: .125rem; margin-left: .75rem; }
  .modal-backdrop { position: fixed; z-index: 80; inset: 0; display: grid; place-items: center; background: #080c0acc; }
  .modal::backdrop { background: #080c0acc; }
  .modal { width: min(28rem,90vw); color: #edf1ee; background: #202823; border: .0625rem solid #ffffff14; border-radius: 1.25rem; padding: 2rem; box-shadow: 0 2rem 6rem #0008; }
  .modal h2 { font-size: 1.5rem; line-height: 1.35; font-weight: 550; }
  .modal p { color: #b1c4b9; font-size: 1rem; line-height: 1.7; }
  .modal > div { display: flex; flex-wrap: wrap; gap: .5rem; margin-top: 1.5rem; }
  .modal button { border: .0625rem solid #ffffff15; background: #ffffff07; color: #d1e1d7; border-radius: .5rem; padding: .625rem .75rem; font-size: .9375rem; }
  .modal .destructive { background: #b54d6820; border-color: #b54d6845; color: #efb3c5; }
  .modal-icon { color: #d493aa; }
  .closing { text-align: center; color: #b1cbbb; }
  .spin { animation: rotate 1.5s linear infinite; }
  @keyframes rotate { to { transform: rotate(360deg); } }
  @keyframes pulse { 50% { opacity: .45; } }
  @media(prefers-reduced-motion:reduce) { :global(*) { animation: none !important; transition: none !important; } }
  @media(max-width:75rem) { .titlebar { gap: .5rem; padding-inline: .5rem; } .header-right,.filters { gap: .375rem; } .platforms button,.segmented button,.window-actions button,.app-actions > button,.refresh-menu summary { width: 2rem; height: 2rem; } main { padding-inline: 1.5rem; } }
  @media(max-width:58rem) { .titlebar { grid-template-columns: minmax(0,1fr) 8rem minmax(0,1fr); } .header-right,.filters { gap: .25rem; } .platforms button,.segmented button,.window-actions button,.app-actions > button,.refresh-menu summary { width: 1.75rem; height: 1.75rem; padding: .25rem; } }
</style>
