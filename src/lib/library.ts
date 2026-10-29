export type OS = 'linux' | 'windows' | 'unknown';
export type Installation = { os: OS; state: string };
export type ListedGame = { key: string; store: 'steam' | 'epic'; title: string; inLibrary: boolean; installations: Installation[] };
export type Filters = { installed: boolean; notInstalled: boolean; otherOS: boolean; steam: boolean; epic: boolean };

export function matchesFilters(game: ListedGame, search: string, filters: Filters, host: OS) {
  if (!game.title.toLocaleLowerCase().includes(search.toLocaleLowerCase())) return false;
  if ((filters.steam || filters.epic) && !filters[game.store]) return false;
  const installed = game.installations.some(i => ['installed', 'external'].includes(i.state));
  const foreign = game.installations.some(i => i.os !== host && i.os !== 'unknown' && i.state !== 'files_missing');
  if (filters.otherOS && !foreign) return false;
  // Installed/not installed are one facet; OS location is a separate constraint.
  // Selecting both statuses (or neither) leaves installation status unrestricted.
  return filters.installed === filters.notInstalled
    || (filters.installed ? installed : !installed);
}
