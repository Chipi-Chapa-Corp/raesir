import { test } from 'node:test';
import assert from 'node:assert/strict';
import { gameForOS, matchesFilters, emptyFilters, stores } from '../src/lib/library.ts';

const empty = emptyFilters;
const local = { key: 'steam:1', store: 'steam', title: 'Local game', inLibrary: true, client_os: ['linux'], installations: [{ os: 'linux', state: 'installed' }] };
const foreign = { key: 'epic:2', store: 'epic', title: 'Other OS game', inLibrary: true, client_os: ['windows'], installations: [{ os: 'windows', state: 'installed' }] };
const uninstalled = { ...local, key: 'steam:3', title: 'Cloud game', installations: [] };
const ea = { ...foreign, key: 'ea:1', store: 'ea', title: 'EA game' };
const match = (game, filters = {}, search = '') => matchesFilters(game, search, { ...empty, ...filters }, 'linux');

test('unchecked filters include installed, uninstalled and foreign games', () => {
  for (const game of [local, foreign, uninstalled, ea]) assert.equal(match(game), true);
});
test('installed includes installations on either OS', () => {
  assert.equal(match(local, { installed: true }), true);
  assert.equal(match(foreign, { installed: true }), true);
  assert.equal(match(uninstalled, { installed: true }), false);
});
test('not installed excludes games installed on another OS', () => {
  assert.equal(match(local, { notInstalled: true }), false);
  assert.equal(match(foreign, { notInstalled: true }), false);
  assert.equal(match(uninstalled, { notInstalled: true }), true);
});
test('other OS follows the client even when its game files are missing', () => {
  assert.equal(match(foreign, { otherOS: true }), true);
  assert.equal(match(local, { otherOS: true }), false);
  assert.equal(match({ ...foreign, installations: [{ os: 'windows', state: 'unavailable' }] }, { otherOS: true }), true);
  assert.equal(match({ ...foreign, installations: [{ os: 'windows', state: 'files_missing' }] }, { otherOS: true }), true);
});
test('installation status, OS location and store narrow each other', () => {
  const filters = { installed: true, otherOS: true, epic: true };
  assert.equal(match(foreign, filters), true);
  assert.equal(match(local, filters), false);
  assert.equal(match(uninstalled, filters), false);
  assert.equal(match(local, { ...filters, steam: true }), false);
});
test('other OS narrows store selections and the installed subset', () => {
  const games = [local, foreign, uninstalled, ea,
    { ...foreign, key: 'epic:4', installations: [{ os: 'windows', state: 'unavailable' }] },
    { ...foreign, key: 'epic:5', installations: [{ os: 'windows', state: 'incomplete' }] },
    { ...local, key: 'steam:6', installations: [...local.installations, ...foreign.installations] }];
  for (const installed of [false, true]) {
    for (const steam of [false, true]) for (const epic of [false, true]) for (const ea of [false, true]) {
      const filters = { installed, steam, epic, ea };
      const before = games.filter(g => match(g, filters));
      const after = games.filter(g => match(g, { ...filters, otherOS: true }));
      assert.ok(after.every(g => before.includes(g)));
    }
  }
});
test('25 local installs plus 3 foreign installs narrow to 3 with other OS', () => {
  const games = [
    ...Array.from({ length: 25 }, (_, i) => ({ ...local, key: `steam:${i}` })),
    ...Array.from({ length: 3 }, (_, i) => ({ ...foreign, key: `epic:${i}` })),
    uninstalled
  ];
  assert.equal(games.filter(g => match(g, { installed: true })).length, 28);
  assert.equal(games.filter(g => match(g, { installed: true, otherOS: true })).length, 3);
});
test('search combines with store and installation filters', () => {
  assert.equal(match(local, { steam: true, installed: true }, 'LOCAL'), true);
  assert.equal(match(local, { epic: true }, 'local'), false);
  assert.equal(match(foreign, { epic: true }, 'local'), false);
});
test('EA store combines with status, OS, search and other stores', () => {
  assert.equal(match(ea, { ea: true, installed: true, otherOS: true }, 'EA'), true);
  assert.equal(match(ea, { epic: true }), false);
  assert.equal(match(local, { ea: true }), false);
  assert.equal(match(ea, { ea: true, notInstalled: true }), false);
  assert.equal(match(local, { ea: true, steam: true }), true);
  assert.equal(match(foreign, { ea: true, steam: true }), false);
});

test('every store can be selected alone or combined without breaking status facets', () => {
  const games = stores.map(store => ({ ...foreign, store, key: `${store}:fixture` }));
  for (const store of stores) {
    assert.deepEqual(games.filter(g => match(g, { [store]: true })).map(g => g.store), [store]);
    assert.equal(games.filter(g => match(g, { [store]: true, installed: true, otherOS: true })).length, 1);
    assert.equal(games.filter(g => match(g, { [store]: true, notInstalled: true })).length, 0);
  }
});

test('OS filters select client libraries independently of installation status', () => {
  const both = { ...local, client_os: ['linux', 'windows'], installations: [] };
  assert.equal(match(local, { thisOS: true }), true);
  assert.equal(match(both, { thisOS: true }), true);
  assert.equal(match(both, { otherOS: true, notInstalled: true }), true);
  assert.equal(match(foreign, { thisOS: true }), false);
  for (const game of [uninstalled, { ...local, installations: [{ os: 'linux', state: 'unavailable' }] }, { ...local, installations: [{ os: 'linux', state: 'incomplete' }] }]) {
    assert.equal(match(game, { thisOS: true }), true);
    assert.equal(match(game, { thisOS: true, notInstalled: true }), true);
    assert.equal(match(game, { thisOS: true, installed: true }), false);
  }
  assert.equal(match(local, { thisOS: true, installed: true, steam: true }, 'local'), true);
  assert.equal(match(local, { thisOS: true, notInstalled: true }), false);
  assert.equal(matchesFilters(local, '', { ...empty, thisOS: true }, 'windows'), false);
});

test('Windows-only Ubisoft libraries stay out of This OS on Linux even for uninstalled games', () => {
  const windows = { ...foreign, key: 'ubisoft:1', store: 'ubisoft', installations: [] };
  const linux = { ...windows, client_os: ['linux'] };
  assert.equal(match(windows, { thisOS: true, ubisoft: true }), false);
  assert.equal(match(windows, { otherOS: true, ubisoft: true, notInstalled: true }), true);
  assert.equal(match(linux, { thisOS: true, ubisoft: true }), true);
  assert.equal(match(linux, { thisOS: true, ubisoft: true, notInstalled: true }), true);
  assert.equal(match(linux, { thisOS: true, ubisoft: true, installed: true }), false);
});

test('This OS uses local install state and badges for owned games also installed on Windows', () => {
  const game = { ...foreign, store: 'steam', client_os: ['linux', 'windows'] };
  const thisOS = { ...empty, thisOS: true };
  assert.equal(match(game, { thisOS: true }), true);
  assert.equal(match(game, { thisOS: true, installed: true }), false);
  assert.equal(match(game, { thisOS: true, notInstalled: true }), true);
  const view = gameForOS(game, thisOS, 'linux');
  assert.deepEqual(view.installations, []);
  assert.deepEqual(view.client_os, ['linux', 'windows']);
  assert.equal(game.installations.length, 1);
  assert.equal(gameForOS(game, empty, 'linux'), game);
  assert.equal(matchesFilters(game, '', { ...thisOS, installed: true }, 'windows'), true);
  assert.deepEqual(gameForOS(game, thisOS, 'windows').installations, game.installations);
});

test('Other OS scopes status and card badges independently from local installs', () => {
  const game = { ...local, client_os: ['linux', 'windows'] };
  assert.equal(match(game, { otherOS: true, installed: true }), false);
  assert.equal(match(game, { otherOS: true, notInstalled: true }), true);
  const otherOS = { ...empty, otherOS: true };
  assert.deepEqual(gameForOS(game, otherOS, 'linux').installations, []);
  const both = { ...game, installations: [...local.installations, ...foreign.installations] };
  assert.deepEqual(gameForOS(both, otherOS, 'linux').installations, foreign.installations);
  assert.deepEqual(gameForOS(both, { ...empty, thisOS: true }, 'linux').installations, local.installations);
});
