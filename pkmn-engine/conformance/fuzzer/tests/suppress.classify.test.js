'use strict';

const test = require('node:test');
const assert = require('node:assert');
const path = require('path');
const fs = require('fs');

const suppress = require('../suppress.js');
const { CLASSIFICATIONS } = require('../constants.js');

test('GUARDED_ITEM_IDS contains 0; GUARDED_MOVE_IDS contains 0 and 150', () => {
  assert.ok(suppress.GUARDED_ITEM_IDS.has(0));
  assert.ok(suppress.GUARDED_MOVE_IDS.has(0));
  assert.ok(suppress.GUARDED_MOVE_IDS.has(150));
});

test('GUARDED_* sets contain exactly the canonical IDs (no drift)', () => {
  // Object.freeze does not block Set.add, and mutating these sets would pollute later tests.
  assert.strictEqual(suppress.GUARDED_ITEM_IDS.size, 1);
  assert.strictEqual(suppress.GUARDED_MOVE_IDS.size, 2);
  assert.deepStrictEqual([...suppress.GUARDED_ITEM_IDS], [0]);
  assert.deepStrictEqual([...suppress.GUARDED_MOVE_IDS].sort((a, b) => a - b), [0, 150]);
});

test('collectFromMon: scalar item_id=0 is skipped', () => {
  const acc = { species: new Set(), abilities: new Set(), items: new Set(), moves: new Set() };
  suppress.collectFromMon({ species_id: 25, ability_id: 9, item_id: 0, move_ids: [33] }, acc);
  assert.ok(!acc.items.has(0));
  assert.strictEqual(acc.items.size, 0);
});

test('collectFromMon: scalar move_id=0 is skipped', () => {
  const acc = { species: new Set(), abilities: new Set(), items: new Set(), moves: new Set() };
  suppress.collectFromMon({ species_id: 25, ability_id: 9, item_id: 100, move_ids: [33, 0, 0, 0] }, acc);
  assert.ok(!acc.moves.has(0));
  assert.ok(acc.moves.has(33));
  assert.strictEqual(acc.moves.size, 1);
});

test('collectFromMon: scalar move_id=150 (Splash) is skipped', () => {
  const acc = { species: new Set(), abilities: new Set(), items: new Set(), moves: new Set() };
  suppress.collectFromMon({ species_id: 25, ability_id: 9, item_id: 100, move_ids: [33, 150] }, acc);
  assert.ok(!acc.moves.has(150));
  assert.ok(acc.moves.has(33));
});

test('collectFromMon: {id, pp} object move shapes — guard applies', () => {
  const acc = { species: new Set(), abilities: new Set(), items: new Set(), moves: new Set() };
  suppress.collectFromMon({
    species_id: 25, ability_id: 9, item_id: 100,
    moves: [{ id: 33, pp: 5 }, { id: 150, pp: 40 }, { id: 0, pp: 0 }],
  }, acc);
  assert.ok(acc.moves.has(33));
  assert.ok(!acc.moves.has(150));
  assert.ok(!acc.moves.has(0));
});

test('collectFromMon: non-guarded items survive (non-vacuous-LOW regression)', () => {
  const acc = { species: new Set(), abilities: new Set(), items: new Set(), moves: new Set() };
  suppress.collectFromMon({ species_id: 25, ability_id: 9, item_id: 217 /* assault vest */, move_ids: [33] }, acc);
  assert.ok(acc.items.has(217));
});

function makeKb({ species = {}, abilities = {}, items = {}, moves = {} } = {}) {
  return { species, abilities, items, moves };
}

test('classify: empty inputs → NONE', () => {
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: {},
    initial_team: { p1: [], p2: [] },
    divergent_turn: null,
    knownBugs: {},
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.NONE);
  assert.deepStrictEqual(r.bug_ids, []);
});

test('classify: fully-flagged lead on divergent turn → HIGH', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const mon = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: mon, p2: mon } },
    initial_team: { p1: [mon], p2: [mon] },
    divergent_turn: 0,
    knownBugs: kb,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.HIGH);
  assert.ok(r.bug_ids.length > 0);
});

test('classify: one unflagged entity in accumulated → LOW', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  // Ability 200 is deliberately absent from kb, so forAll fails while overlap survives.
  const leadMon = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const benchMon = { species_id: 25, ability_id: 200, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: leadMon, p2: leadMon } },
    initial_team: { p1: [leadMon, benchMon], p2: [leadMon] },
    divergent_turn: 0,
    knownBugs: kb,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.LOW);
});

test('classify: positive flip — guarded item_id=0 on lead no longer blocks forAll', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const mon = { species_id: 25, ability_id: 9, item_id: 0, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: mon, p2: mon } },
    initial_team: { p1: [mon], p2: [mon] },
    divergent_turn: 0,
    knownBugs: kb,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.HIGH);
});

test('classify: positive flip — guarded move_id=150 (Splash) no longer blocks forAll', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const mon = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33, 150] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: mon, p2: mon } },
    initial_team: { p1: [mon], p2: [mon] },
    divergent_turn: 0,
    knownBugs: kb,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.HIGH);
});

test('classify: non-vacuous LOW — unflagged non-guarded entity still blocks forAll', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const mon = { species_id: 25, ability_id: 9, item_id: 999, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: mon, p2: mon } },
    initial_team: { p1: [mon], p2: [mon] },
    divergent_turn: 0,
    knownBugs: kb,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.LOW);
});

test('participatingFilter: baseline LOW with unflagged bench, omitted filter', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const lead = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const bench = { species_id: 26, ability_id: 200, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: lead, p2: lead } },
    initial_team: { p1: [lead, bench], p2: [lead] },
    divergent_turn: 0,
    knownBugs: kb,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.LOW);
});

test('participatingFilter: filter that drops bench flips LOW → HIGH', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const lead = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const bench = { species_id: 26, ability_id: 200, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: lead, p2: lead } },
    initial_team: { p1: [lead, bench], p2: [lead] },
    divergent_turn: 0,
    knownBugs: kb,
    participatingFilter: (mon) => mon.species_id === 25,
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.HIGH);
});

test('participatingFilter: does NOT gate turn-entry accumulation', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const lead = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: lead, p2: lead } },
    initial_team: { p1: [lead], p2: [lead] },
    divergent_turn: 0,
    knownBugs: kb,
    participatingFilter: () => false,
  });
  // Turn entry alone forms accumulated; entries are flagged; forAll holds.
  assert.strictEqual(r.level, CLASSIFICATIONS.HIGH);
});

test('participatingFilter: throwing filter degrades to admit-all', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const lead = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const bench = { species_id: 26, ability_id: 200, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: lead, p2: lead } },
    initial_team: { p1: [lead, bench], p2: [lead] },
    divergent_turn: 0,
    knownBugs: kb,
    participatingFilter: () => { throw new Error('boom'); },
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.LOW); // admit-all → bench still unflagged
});

test('participatingFilter: non-function value preserves legacy behavior', () => {
  const kb = makeKb({
    species: { '25': ['BUG-T-001'] },
    abilities: { '9': ['BUG-T-002'] },
    items: { '100': ['BUG-T-003'] },
    moves: { '33': ['BUG-T-004'] },
  });
  const lead = { species_id: 25, ability_id: 9, item_id: 100, move_ids: [33] };
  const bench = { species_id: 26, ability_id: 200, item_id: 100, move_ids: [33] };
  const r = suppress.classify({
    signature: null,
    entity_sets_by_turn: { 0: { p1: lead, p2: lead } },
    initial_team: { p1: [lead, bench], p2: [lead] },
    divergent_turn: 0,
    knownBugs: kb,
    participatingFilter: 'not a function',
  });
  assert.strictEqual(r.level, CLASSIFICATIONS.LOW);
});
