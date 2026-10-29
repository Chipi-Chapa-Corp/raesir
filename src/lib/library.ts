export type OS = 'linux' | 'windows' | 'unknown';
export type Installation = { os: OS; state: string };
export type Store = 'steam' | 'epic' | 'ea' | 'ubisoft' | 'xbox';
export type ListedGame = { key: string; store: Store; title: string; inLibrary: boolean; installations: Installation[]; client_os?: OS[] };
export type Filters = { installed: boolean; notInstalled: boolean; otherOS: boolean; thisOS: boolean; steam: boolean; epic: boolean; ea: boolean; ubisoft: boolean; xbox: boolean };
export type GameAction = { client: string; target: unknown; note?: string | null };
export type Game = {
  key: string; store: Store; id: string; title: string; in_library: boolean;
  installations: Installation[]; client: string; client_os?: OS[]; cover: string | null;
  launch: GameAction | null; store_page: GameAction | null; uninstall: GameAction | null;
};
export const storeNames: Record<Store, string> = { steam: 'Steam', epic: 'Epic Games', ea: 'EA app', ubisoft: 'Ubisoft Connect', xbox: 'Xbox' };
export const stores: Store[] = ['steam', 'epic', 'ea', 'ubisoft', 'xbox'];
export const emptyFilters: Filters = { installed: false, notInstalled: false, otherOS: false, thisOS: false, steam: false, epic: false, ea: false, ubisoft: false, xbox: false };
export const installedHere = (game: Game, host: OS) => game.installations.some(i => i.os === host && ['installed', 'external'].includes(i.state));
export const gameView = (game: Game): ListedGame => ({ ...game, inLibrary: game.in_library });
export function installationsForOS(installations: Installation[], filters: Filters, host: OS) {
  if (!filters.thisOS && !filters.otherOS) return installations;
  return installations.filter(i => (!filters.thisOS || (host !== 'unknown' && i.os === host))
    && (!filters.otherOS || (i.os !== 'unknown' && i.os !== host)));
}
export function gameForOS(game: Game, filters: Filters, host: OS): Game {
  const installations = installationsForOS(game.installations, filters, host);
  return installations === game.installations ? game : { ...game, installations };
}

export function matchesFilters(game: ListedGame, search: string, filters: Filters, host: OS) {
  if (!game.title.toLocaleLowerCase().includes(search.toLocaleLowerCase())) return false;
  if (stores.some(s => filters[s]) && !filters[game.store]) return false;
  const installed = installationsForOS(game.installations, filters, host).some(i => ['installed', 'external'].includes(i.state));
  const systems = game.client_os ?? game.installations.map(i => i.os);
  if (filters.otherOS && !systems.some(os => os !== host && os !== 'unknown')) return false;
  if (filters.thisOS && (host === 'unknown' || !systems.includes(host))) return false;
  // Installed/not installed are one facet; OS location is a separate constraint.
  // Selecting both statuses (or neither) leaves installation status unrestricted.
  return filters.installed === filters.notInstalled
    || (filters.installed ? installed : !installed);
}
