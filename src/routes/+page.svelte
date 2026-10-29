<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  type OS = 'linux' | 'windows' | 'unknown';
  type Installation = { library: string; path: string; os: OS; state: string; last_seen: number };
  type Report = {
    host_os: OS;
    games: { appid: number; title: string; owned: boolean; installations: Installation[] }[];
    libraries: { path: string; os: OS }[];
    volumes: { id: string; device: string; filesystem: string; mountpoints: string[]; os: OS; note: string }[];
    warnings: string[];
    owned_account: string | null;
    owned_source: string;
  };
  type Source = { path: string; os: OS };
  type Mapping = { original: string; mounted: string };
  type Session = { clients: { root: string; label: string }[]; managed_root: string | null; external_running: boolean; supported: boolean };

  let report = $state<Report | null>(null);
  let busy = $state(false);
  let error = $state('');
  let sources = $state<Source[]>([]);
  let mappings = $state<Mapping[]>([]);
  let newPath = $state('');
  let newOS = $state<OS>('unknown');
  let original = $state('');
  let mounted = $state('');
  let steamid = $state('');
  let apiKey = $state('');
  let search = $state('');
  let session = $state<Session | null>(null);
  let clientRoot = $state('');
  let notice = $state('');
  let closing = $state(false);
  let settingsReady = false;
  const labels: Record<OS, string> = { linux: 'Linux', windows: 'Windows', unknown: 'OS unknown' };
  const games = $derived(report?.games.filter(g => g.title.toLowerCase().includes(search.toLowerCase())) ?? []);

  function options() { return { sources: $state.snapshot(sources), mappings: $state.snapshot(mappings) }; }
  async function refreshSession() {
    try {
      session = await invoke<Session>('steam_status');
      if (!clientRoot) clientRoot = session.managed_root ?? session.clients[0]?.root ?? '';
    } catch (e) { error = String(e); }
  }
  function save() {
    if (!settingsReady) return;
    try { localStorage.setItem('raesir.scan-options.v1', JSON.stringify(options())); }
    catch { error = 'Could not remember these paths. They still apply to this session.'; }
  }
  async function request(command = 'scan_steam', args: Record<string, unknown> = {}) {
    if (busy) return;
    busy = true; error = ''; notice = '';
    try {
      report = await invoke<Report>(command, { options: options(), ...args });
      if (report.owned_account) steamid = report.owned_account;
    } catch (e) { error = String(e); }
    finally { busy = closing; }
  }
  async function readClientLibrary() {
    await request('import_client_library', { root: clientRoot });
    await refreshSession();
  }
  async function stopSteam() {
    if (busy) return;
    busy = true; error = ''; notice = '';
    try { await invoke('stop_steam'); notice = 'Managed Steam session stopped.'; }
    catch (e) { error = String(e); }
    finally { busy = closing; await refreshSession(); }
  }
  async function mountVolume(id: string) {
    await request('mount_volume', { id });
    if (!error) notice = 'Partition mounted read-only. Steam libraries refreshed.';
  }
  function addSource(event: SubmitEvent) {
    event.preventDefault();
    const path = newPath.trim();
    if (!path) return;
    if (!sources.some(s => s.path === path && s.os === newOS)) sources = [...sources, { path, os: newOS }];
    newPath = ''; save(); void request();
  }
  function addMapping(event: SubmitEvent) {
    event.preventDefault();
    if (!original.trim() || !mounted.trim()) return;
    mappings = [...mappings.filter(m => m.original !== original.trim()), { original: original.trim(), mounted: mounted.trim() }];
    original = ''; mounted = ''; save(); void request();
  }
  async function importOwned(event: SubmitEvent) {
    event.preventDefault();
    const key = apiKey.trim(); apiKey = '';
    await request('import_owned', { steamid: steamid.trim(), apiKey: key });
  }
  function installationLabel(i: Installation) {
    const os = labels[i.os];
    if (i.state === 'unavailable') return `Previously installed on ${os} · location unavailable`;
    if (i.state === 'files_missing') return `Manifest on ${os} · game folder missing`;
    if (i.state === 'incomplete') return `Incomplete installation on ${os}`;
    return `Installed on ${os}`;
  }
  onMount(() => {
    let disposed = false;
    const listeners = [
      listen('steam-shutting-down', () => { closing = true; busy = true; notice = 'Stopping the managed Steam session before closing…'; }),
      listen<string>('steam-shutdown-error', event => { closing = false; busy = false; error = event.payload; notice = ''; })
    ];
    for (const listener of listeners) void listener.then(unlisten => { if (disposed) unlisten(); }).catch(e => { error = String(e); });
    try {
      const saved = JSON.parse(localStorage.getItem('raesir.scan-options.v1') ?? '{}');
      if (Array.isArray(saved.sources)) sources = saved.sources.filter((s: Source) => typeof s.path === 'string' && ['linux', 'windows', 'unknown'].includes(s.os));
      if (Array.isArray(saved.mappings)) mappings = saved.mappings.filter((m: Mapping) => typeof m.original === 'string' && typeof m.mounted === 'string');
    } catch { error = 'Saved scan settings could not be read.'; }
    settingsReady = true;
    void request();
    void refreshSession();
    return () => { disposed = true; for (const listener of listeners) void listener.then(unlisten => unlisten()).catch(() => {}); };
  });
</script>

<svelte:head><title>Raesir — Steam library</title></svelte:head>

<main>
  <header>
    <div><h1>Raesir</h1><p>Steam library{report ? ` · ${labels[report.host_os]}` : ''}</p></div>
    <button disabled={busy} onclick={() => request()}>{busy ? 'Scanning…' : 'Rescan'}</button>
  </header>

  <p class="muted">Games on another OS stay visible. Mount a partition read-only or start a managed Steam session when needed.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p role="status">{notice}</p>{/if}

  <details>
    <summary>Other OS & extra folders ({sources.length})</summary>
    <p>Add a Steam folder, Steam library, or a readable OS root. Choose the OS that manages that installation; a Windows game running through Proton can still belong to Linux Steam.</p>
    <form onsubmit={addSource}>
      <label>Folder path<input bind:value={newPath} placeholder="/mnt/windows or D:\SteamLibrary" required /></label>
      <label>Installation OS<select bind:value={newOS}><option value="unknown">Auto / unknown</option><option value="windows">Windows</option><option value="linux">Linux</option></select></label>
      <button disabled={busy}>Add & scan</button>
    </form>
    {#if sources.length}<ul>{#each sources as source, index}<li><code>{source.path}</code> · {labels[source.os]} <button class="small" disabled={busy} onclick={() => { sources = sources.filter((_, i) => i !== index); save(); void request(); }}>Remove</button></li>{/each}</ul>{/if}
    <p>Map paths recorded by the other OS to their current location, for example <code>D:</code> → <code>/mnt/games</code> or <code>/home</code> → <code>L:\home</code>.</p>
    <form onsubmit={addMapping}>
      <label>Original prefix<input bind:value={original} placeholder="D:" required /></label>
      <label>Mounted prefix<input bind:value={mounted} placeholder="/mnt/games" required /></label>
      <button disabled={busy}>Add mapping</button>
    </form>
    {#if mappings.length}<ul>{#each mappings as mapping, index}<li><code>{mapping.original}</code> → <code>{mapping.mounted}</code> <button class="small" disabled={busy} onclick={() => { mappings = mappings.filter((_, i) => i !== index); save(); void request(); }}>Remove</button></li>{/each}</ul>{/if}
  </details>

  <details>
    <summary>Steam library & ownership</summary>
    <p>Use Steam's existing sign-in without entering an API key. Raesir uses a running Steam client, or starts Steam minimized if needed. Finish any login prompt in Steam and retry if needed.</p>
    <p>The client bridge is experimental and checks licenses against Steam's cached game catalog. Shared or temporary access is shown as <strong>In Steam library</strong>, not permanent ownership.</p>
    {#if session?.supported && session.clients.length}
      <label>Steam installation<select bind:value={clientRoot} disabled={busy || !!session.managed_root}>{#each session.clients as client}<option value={client.root}>{client.label}</option>{/each}</select></label>
      <p><button disabled={busy || !clientRoot} onclick={readClientLibrary}>{session.managed_root ? 'Read Steam library again' : session.external_running ? 'Read running Steam library' : 'Start Steam & read library'}</button> <button disabled={busy || !session.managed_root} onclick={stopSteam}>Stop managed Steam</button></p>
      {#if session.external_running}<p class="muted">Steam is already running. Raesir will use its session and leave it running when Raesir closes.</p>{/if}
    {:else}<p class="muted">{session?.supported ? 'No launchable Steam client was detected.' : 'The client-auth bridge currently supports Linux x86_64. Use the Web API below on other platforms.'}</p>{/if}
    <p class="muted">Only Steam started by Raesir is shut down before Raesir closes. An already-running Steam client stays under your control. Forced app termination or a system crash can bypass normal cleanup.</p>
    <details><summary>Use a Steam Web API key instead</summary>
    <p>Installed manifests do not prove ownership. To include owned games without a known installation, look up one account using the Steam Web API. Its Game details must be visible. The API key is sent only to Steam and is not saved. Library results are cached locally.</p>
    <form onsubmit={importOwned}>
      <label>SteamID64<input bind:value={steamid} placeholder="17-digit SteamID64" inputmode="numeric" required /></label>
      <label>Steam Web API key<input bind:value={apiKey} type="password" autocomplete="off" required /></label>
      <button disabled={busy}>Look up owned games</button>
    </form>
    </details>
    {#if report?.owned_account}<p>Cached account: <code>{report.owned_account}</code>. Ownership reflects the last successful lookup. <button class="small" disabled={busy} onclick={() => request('forget_owned')}>Forget owned list</button></p>{/if}
  </details>

  {#if report}
    <details class:warning={report.warnings.length > 0 || report.volumes.some(v => v.mountpoints.length === 0)}>
      <summary>Storage & scan warnings ({report.warnings.length + report.volumes.filter(v => v.mountpoints.length === 0).length})</summary>
      {#if report.warnings.length}<ul>{#each report.warnings as warning}<li>{warning}</li>{/each}</ul>{/if}
      {#if !report.volumes.length}<p>No partitions could be listed. Readable paths can still be added above.</p>{/if}
      <p>“Mount read-only” asks the OS to mount that partition, then refreshes the library. Your desktop may ask for approval or an administrator password; Raesir does not receive it. Encrypted volumes must first be unlocked with your OS tools.</p>
      <ul>{#each report.volumes as volume}<li><strong>{volume.filesystem}</strong> · <code>{volume.device}</code> · {labels[volume.os]}<div>{volume.note}</div>{#if report.host_os === 'linux' && !volume.mountpoints.length && ['ntfs', 'ext4', 'btrfs'].includes(volume.filesystem.toLowerCase())}<button class="small" disabled={busy} onclick={() => mountVolume(volume.id)}>Mount read-only</button>{/if}{#if volume.mountpoints.length}<div class="muted">{volume.mountpoints.join(', ')}</div>{/if}</li>{/each}</ul>
      <p class="muted">A filesystem alone does not prove another OS is installed. OS labels come from readable OS markers or your selected installation OS.</p>
    </details>

    <section aria-labelledby="games-title" aria-busy={busy}>
      <h2 id="games-title">Steam titles ({report.games.length})</h2>
      <p class="muted">{report.libraries.length} readable libraries. {report.owned_account ? (report.owned_source === 'steam_client' ? 'Steam library access reflects the last client lookup against its cached catalog.' : 'Owned titles include the cached Steam account library.') : 'Ownership unknown until an account library is loaded.'}</p>
      <label class="filter">Filter titles<input type="search" bind:value={search} placeholder="Title…" /></label>
      {#if games.length}
        <ul class="games">{#each games as game (game.appid)}
          <li><strong>{game.title}</strong><span class="muted"> · {game.owned ? (report.owned_source === 'steam_client' ? 'In Steam library' : 'Owned') : 'Ownership unknown'} · {game.appid}</span>
            {#each game.installations as installation}
              <div class:unavailable={installation.state !== 'installed'}>{installationLabel(installation)}<span class="muted">{installation.os === report.host_os ? ' · this OS' : ''}</span></div>
              <div class="path"><code>{installation.path}</code>{#if installation.state === 'unavailable'} · last seen {new Date(installation.last_seen * 1000).toLocaleString()}{/if}</div>
            {/each}
            {#if !game.installations.length}<div>{report.owned_source === 'steam_client' ? 'In Steam library' : 'Owned'} · no installation found in scanned libraries</div>{/if}
          </li>
        {/each}</ul>
      {:else}<p>{search ? 'No titles match this filter.' : 'No Steam titles found. Add a Steam folder or an accessible other-OS root above.'}</p>{/if}
    </section>
    <details><summary>Scanned libraries</summary><ul>{#each report.libraries as library}<li><code>{library.path}</code> · {labels[library.os]}</li>{/each}</ul></details>
  {:else}<p role="status">{busy ? 'Scanning Steam libraries…' : 'No scan results yet.'}</p>{/if}
</main>

<style>
  :global(body) { margin: 0; background: #fafafa; color: #222; font-family: system-ui, sans-serif; font-size: 14px; line-height: 1.5; }
  :global(*) { box-sizing: border-box; }
  main { max-width: 1000px; margin: 0 auto; padding: 24px; }
  header { display: flex; justify-content: space-between; align-items: center; gap: 16px; }
  h1 { margin: 0; font-size: 25px; } h2 { font-size: 18px; margin-bottom: 4px; }
  header p { margin: 0; } p { margin: 10px 0; }
  button, input, select { font: inherit; border: 1px solid #aaa; border-radius: 3px; padding: 6px 9px; background: white; color: inherit; }
  button { cursor: pointer; } button:disabled { opacity: .55; cursor: wait; }
  button:hover:enabled { background: #eee; } button:focus-visible, input:focus-visible, select:focus-visible, summary:focus-visible { outline: 2px solid #356cab; outline-offset: 2px; }
  form { display: flex; gap: 8px; align-items: end; flex-wrap: wrap; margin: 14px 0; }
  label { display: flex; flex-direction: column; gap: 3px; flex: 1; min-width: 140px; }
  input { min-width: 0; width: 100%; }
  details { border: 1px solid #ddd; padding: 10px 12px; margin: 12px 0; border-radius: 3px; }
  summary { cursor: pointer; font-weight: 600; }
  .warning { border-color: #c3a364; background: #fffaf0; }
  li { margin: 7px 0; overflow-wrap: anywhere; }
  .games { padding-left: 22px; } .games > li { margin: 14px 0; }
  .muted, .path { color: #666; } .path { font-size: 12px; } code { font-family: ui-monospace, monospace; }
  .error { border: 1px solid #b54b42; color: #8f221b; padding: 10px; }
  .unavailable { color: #8a5712; } .small { padding: 1px 6px; font-size: 12px; }
  .filter { max-width: 350px; }
</style>
