<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { matchesFilters, type Filters, type ListedGame, type Installation, type OS } from '$lib/library';

  type Report = {
    host_os: OS;
    games: { appid: number; title: string; owned: boolean; installations: Installation[] }[];
    volumes: { id: string; filesystem: string; mountpoints: string[]; os: OS }[];
    warnings: string[];
    epic: {
      games: { app_name: string; title: string; library_sources: string[]; installations: Installation[] }[];
      sources: { path: string; kind: string; local: boolean }[];
      legendary_available: boolean;
    };
  };
  type Session = { clients: { root: string }[]; managed_root: string | null; supported: boolean };
  let report = $state<Report | null>(null);
  let busy = $state(false);
  let closing = $state(false);
  let error = $state('');
  let notice = $state('');
  let search = $state('');
  let filters = $state<Filters>({ installed: false, notInstalled: false, otherOS: false, steam: false, epic: false });
  const labels: Record<OS, string> = { linux: 'Linux', windows: 'Windows', unknown: 'another OS' };
  const allGames = $derived<ListedGame[]>([
    ...(report?.games.map(g => ({ key: `steam:${g.appid}`, store: 'steam' as const, title: g.title,
      inLibrary: g.owned, installations: g.installations })) ?? []),
    ...(report?.epic.games.map(g => ({ key: `epic:${g.app_name}`, store: 'epic' as const, title: g.title,
      inLibrary: !!g.library_sources.length, installations: g.installations })) ?? [])
  ].sort((a, b) => a.title.localeCompare(b.title) || a.key.localeCompare(b.key)));
  const games = $derived(allGames.filter(g => matchesFilters(g, search, filters, report?.host_os ?? 'unknown')));
  const mounts = $derived(report?.volumes.filter(v => !v.mountpoints.length && ['ntfs', 'ext4', 'btrfs'].includes(v.filesystem.toLowerCase())) ?? []);

  async function scan(command = 'scan_library', args: Record<string, unknown> = {}) {
    if (busy) return;
    busy = true; error = ''; notice = '';
    try {
      report = await invoke<Report>(command, args);
      if (closing) return;
      const steam = await invoke<Session>('steam_status');
      const root = steam.managed_root ?? steam.clients[0]?.root;
      if (steam.supported && root) {
        try { report = await invoke<Report>('import_client_library', { root }); }
        catch { notice = 'Steam ownership could not be refreshed. Finish signing in to Steam, then rescan.'; }
      }
    } catch (e) { error = String(e); }
    finally { busy = closing; }
  }
  async function refreshEpic() {
    if (busy || !report) return;
    const profiles = report.epic.sources.filter(s => s.kind === 'legendary' && s.local);
    if (!profiles.length || !report.epic.legendary_available) {
      error = 'Sign in through Heroic or Legendary and install the Legendary CLI to refresh Epic online.';
      return;
    }
    busy = true; error = ''; notice = '';
    try {
      for (const profile of profiles) {
        if (closing) break;
        report = await invoke<Report>('refresh_epic', { profile: profile.path });
      }
    } catch (e) { error = String(e); }
    finally { busy = closing; }
  }
  function installationState(game: ListedGame) {
    const local = game.installations.filter(i => i.os === report?.host_os);
    const foreign = [...new Set(game.installations.filter(i => i.os !== report?.host_os && i.os !== 'unknown' && ['installed', 'incomplete', 'external'].includes(i.state)).map(i => labels[i.os]))];
    const previous = [...new Set(game.installations.filter(i => i.os !== report?.host_os && i.os !== 'unknown' && i.state === 'unavailable').map(i => labels[i.os]))];
    const parts: string[] = [];
    if (local.some(i => i.state === 'installed')) parts.push('Installed');
    else if (local.some(i => i.state === 'external')) parts.push('Recorded in another client');
    else if (local.some(i => i.state === 'incomplete')) parts.push('Installation incomplete');
    else if (local.length) parts.push('Installation unavailable');
    if (foreign.length) parts.push(`On ${foreign.join(', ')}`);
    if (previous.length) parts.push(`Previously on ${previous.join(', ')}`);
    if (!parts.length) parts.push('Not installed');
    return parts.join(' · ');
  }
  function mountName(os: OS, filesystem: string) {
    if (os !== 'unknown') return `${labels[os]} detected`;
    return filesystem.toLowerCase() === 'ntfs' ? 'Possible Windows installation detected' : 'Possible Linux installation detected';
  }
  onMount(() => {
    let disposed = false;
    const listeners = [
      listen('app-shutting-down', () => { closing = true; busy = true; notice = 'Closing Steam and unmounting partitions…'; }),
      listen<string>('app-shutdown-error', event => { closing = false; busy = false; error = event.payload; notice = ''; })
    ];
    for (const listener of listeners) void listener.then(unlisten => { if (disposed) unlisten(); }).catch(e => { error = String(e); });
    // Old manual folder settings are intentionally not restored or scanned.
    void scan();
    return () => { disposed = true; for (const listener of listeners) void listener.then(unlisten => unlisten()).catch(() => {}); };
  });
</script>

<svelte:head><title>Raesir — Game library</title></svelte:head>
<main>
  <header><h1>Raesir</h1><div class="actions"><button disabled={busy} onclick={() => scan()}>{busy ? 'Scanning…' : 'Rescan'}</button><button disabled={busy} onclick={refreshEpic}>Refresh Epic</button></div></header>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="muted" role="status">{notice}</p>{/if}
  {#each mounts as volume}
    <div class="banner"><span>{mountName(volume.os, volume.filesystem)}{#if report?.host_os === 'windows'} · {volume.filesystem === 'btrfs' ? 'WinBtrfs support required' : 'Filesystem support required'}{/if}</span><button disabled={busy || report?.host_os !== 'linux'} onclick={() => scan('mount_volume', { id: volume.id })}>Mount</button></div>
  {/each}
  <label class="search">Search games<input type="search" bind:value={search} placeholder="Search…" /></label>
  <div class="filters" role="group" aria-label="Filter games">
    <label><input type="checkbox" bind:checked={filters.installed} />Installed</label>
    <label><input type="checkbox" bind:checked={filters.notInstalled} />Not installed</label>
    <label><input type="checkbox" bind:checked={filters.otherOS} />On a different OS</label>
    <label><input type="checkbox" bind:checked={filters.steam} />Steam</label>
    <label><input type="checkbox" bind:checked={filters.epic} />Epic</label>
  </div>
  <p class="muted" aria-live="polite">{games.length} {games.length === 1 ? 'game' : 'games'}</p>
  <ul aria-busy={busy}>
    {#each games as game (game.key)}
      <li><strong>{game.title}</strong><span class="muted"> · {game.store === 'steam' ? 'Steam' : 'Epic'} · {installationState(game)}{game.inLibrary ? ' · In library' : ''}</span></li>
    {:else}<li class="empty">{busy ? 'Reading your libraries…' : 'No games found.'}</li>{/each}
  </ul>
</main>

<style>
  :global(body) { margin: 0; background: #fafafa; color: #222; font: 14px/1.5 system-ui, sans-serif; }
  :global(*) { box-sizing: border-box; }
  main { max-width: 960px; margin: 0 auto; padding: 24px; }
  header, .banner, .actions { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  h1 { margin: 0; font-size: 25px; } header { margin-bottom: 24px; }
  button, input { font: inherit; } button, .search input { border: 1px solid #aaa; border-radius: 3px; padding: 6px 10px; background: white; color: inherit; }
  button { cursor: pointer; } button:disabled { opacity: .55; cursor: default; }
  .search { display: flex; flex-direction: column; gap: 4px; margin-top: 24px; } .search input { width: 100%; }
  .filters { display: flex; flex-wrap: wrap; gap: 12px 20px; margin: 12px 0; } .filters label { display: flex; align-items: center; gap: 5px; }
  .banner { padding: 10px 12px; margin: 10px 0; background: #fffaf0; border: 1px solid #c3a364; border-radius: 3px; }
  .muted { color: #666; } .error { color: #8f221b; border: 1px solid #b54b42; padding: 10px; }
  li { margin: 12px 0; overflow-wrap: anywhere; } .empty { list-style: none; color: #666; }
  button:focus-visible, input:focus-visible { outline: 2px solid #356cab; outline-offset: 2px; }
</style>
