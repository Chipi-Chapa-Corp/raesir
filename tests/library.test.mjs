import { test } from 'node:test';
import assert from 'node:assert/strict';
import { matchesFilters } from '../src/lib/library.ts';

const empty = { installed: false, notInstalled: false, otherOS: false, steam: false, epic: false };
const local = { key: 'steam:1', store: 'steam', title: 'Local game', inLibrary: true, installations: [{ os: 'linux', state: 'installed' }] };
const foreign = { key: 'epic:2', store: 'epic', title: 'Other OS game', inLibrary: true, installations: [{ os: 'windows', state: 'installed' }] };
const uninstalled = { ...local, key: 'steam:3', title: 'Cloud game', installations: [] };
const match = (game, filters = {}, search = '') => matchesFilters(game, search, { ...empty, ...filters }, 'linux');

test('unchecked filters include installed, uninstalled and foreign games', () => {
  for (const game of [local, foreign, uninstalled]) assert.equal(match(game), true);
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
test('other OS includes inaccessible historical installs but excludes confirmed missing folders', () => {
  assert.equal(match(foreign, { otherOS: true }), true);
  assert.equal(match(local, { otherOS: true }), false);
  assert.equal(match({ ...foreign, installations: [{ os: 'windows', state: 'unavailable' }] }, { otherOS: true }), true);
  assert.equal(match({ ...foreign, installations: [{ os: 'windows', state: 'files_missing' }] }, { otherOS: true }), false);
});
test('installation status, OS location and store narrow each other', () => {
  const filters = { installed: true, otherOS: true, epic: true };
  assert.equal(match(foreign, filters), true);
  assert.equal(match(local, filters), false);
  assert.equal(match(uninstalled, filters), false);
  assert.equal(match(local, { ...filters, steam: true }), false);
});
test('adding other OS never expands any status or store selection', () => {
  const games = [local, foreign, uninstalled,
    { ...foreign, key: 'epic:4', installations: [{ os: 'windows', state: 'unavailable' }] },
    { ...foreign, key: 'epic:5', installations: [{ os: 'windows', state: 'incomplete' }] },
    { ...local, key: 'steam:6', installations: [...local.installations, ...foreign.installations] }];
  for (const installed of [false, true]) for (const notInstalled of [false, true]) {
    for (const steam of [false, true]) for (const epic of [false, true]) {
      const filters = { installed, notInstalled, steam, epic };
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
