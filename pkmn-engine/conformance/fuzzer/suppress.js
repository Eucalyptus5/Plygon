'use strict';

const fs = require('fs');
const { CLASSIFICATIONS } = require('./constants.js');

// Filler ids. Move 0 is bound in known_divergences.json, so collecting it would suppress almost anything.
const GUARDED_ITEM_IDS = Object.freeze(new Set([0]));
const GUARDED_MOVE_IDS = Object.freeze(new Set([0, 150]));

function loadKnownBugs(path) {
  try {
    const raw = fs.readFileSync(path, 'utf8');
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === 'object') return parsed;
    return {};
  } catch (_e) {
    return {};
  }
}

function axis(knownBugs, name) {
  const a = knownBugs && knownBugs[name];
  if (!a || typeof a !== 'object') return {};
  return a;
}

function lookup(axisObj, id) {
  if (id === undefined || id === null) return null;
  const v = axisObj[String(id)];
  if (Array.isArray(v) && v.length > 0) return v;
  return null;
}

function collectFromMon(mon, acc) {
  if (!mon || typeof mon !== 'object') return;
  if (mon.species_id !== undefined && mon.species_id !== null) {
    acc.species.add(mon.species_id);
  }
  if (mon.ability_id !== undefined && mon.ability_id !== null) {
    acc.abilities.add(mon.ability_id);
  }
  if (mon.item_id !== undefined && mon.item_id !== null
      && !GUARDED_ITEM_IDS.has(mon.item_id)) {
    acc.items.add(mon.item_id);
  }
  const moves = Array.isArray(mon.move_ids) ? mon.move_ids
              : Array.isArray(mon.moves) ? mon.moves
              : null;
  if (moves) {
    for (const mv of moves) {
      if (mv === undefined || mv === null) continue;
      // moves entries from scenario JSON team list may be {id, pp} objects.
      if (typeof mv === 'object') {
        if (mv.id !== undefined && mv.id !== null
            && !GUARDED_MOVE_IDS.has(mv.id)) {
          acc.moves.add(mv.id);
        }
      } else if (!GUARDED_MOVE_IDS.has(mv)) {
        acc.moves.add(mv);
      }
    }
  }
}

function collectFromTurnEntry(entry, acc) {
  if (!entry || typeof entry !== 'object') return;
  const sides = ['p1', 'p2'];
  for (const s of sides) {
    const sideMons = entry[s];
    if (Array.isArray(sideMons)) {
      for (const m of sideMons) collectFromMon(m, acc);
    } else if (sideMons && typeof sideMons === 'object') {
      collectFromMon(sideMons, acc);
    }
  }
  if (Array.isArray(entry.mons)) {
    for (const m of entry.mons) collectFromMon(m, acc);
  }
}

function emptyEntitySet() {
  return {
    species: new Set(),
    abilities: new Set(),
    items: new Set(),
    moves: new Set(),
  };
}

function isEmpty(set) {
  return set.species.size === 0
      && set.abilities.size === 0
      && set.items.size === 0
      && set.moves.size === 0;
}

function flaggedSubset(entitySet, knownBugs, sink) {
  const flagged = emptyEntitySet();
  const speciesAxis = axis(knownBugs, 'species');
  const abilitiesAxis = axis(knownBugs, 'abilities');
  const itemsAxis = axis(knownBugs, 'items');
  const movesAxis = axis(knownBugs, 'moves');

  for (const id of entitySet.species) {
    const hits = lookup(speciesAxis, id);
    if (hits) {
      flagged.species.add(id);
      if (sink) for (const b of hits) sink.add(b);
    }
  }
  for (const id of entitySet.abilities) {
    const hits = lookup(abilitiesAxis, id);
    if (hits) {
      flagged.abilities.add(id);
      if (sink) for (const b of hits) sink.add(b);
    }
  }
  for (const id of entitySet.items) {
    const hits = lookup(itemsAxis, id);
    if (hits) {
      flagged.items.add(id);
      if (sink) for (const b of hits) sink.add(b);
    }
  }
  for (const id of entitySet.moves) {
    const hits = lookup(movesAxis, id);
    if (hits) {
      flagged.moves.add(id);
      if (sink) for (const b of hits) sink.add(b);
    }
  }
  return flagged;
}

function forAllFlagged(entitySet, flagged) {
  for (const id of entitySet.species)   if (!flagged.species.has(id))   return false;
  for (const id of entitySet.abilities) if (!flagged.abilities.has(id)) return false;
  for (const id of entitySet.items)     if (!flagged.items.has(id))     return false;
  for (const id of entitySet.moves)     if (!flagged.moves.has(id))     return false;
  return true;
}

function divergentOverlapsFlagged(divergent, flagged) {
  for (const id of divergent.species)   if (flagged.species.has(id))   return true;
  for (const id of divergent.abilities) if (flagged.abilities.has(id)) return true;
  for (const id of divergent.items)     if (flagged.items.has(id))     return true;
  for (const id of divergent.moves)     if (flagged.moves.has(id))     return true;
  return false;
}

function classify({ signature, entity_sets_by_turn, initial_team, divergent_turn, knownBugs, participatingFilter }) {
  const kb = knownBugs && typeof knownBugs === 'object' ? knownBugs : {};
  // entity_sets_by_turn is keyed by 0-based turn index; an Array shape is tolerated.
  const turns = (entity_sets_by_turn && typeof entity_sets_by_turn === 'object')
    ? entity_sets_by_turn
    : {};
  const dt = Number.isInteger(divergent_turn) ? divergent_turn : -1;

  // The filter gates only the initial_team augmentation and fails open, so a bad predicate cannot mute a divergence.
  const haveFilter = typeof participatingFilter === 'function';
  function admit(mon) {
    if (!haveFilter) return true;
    try { return participatingFilter(mon) !== false; }
    catch (_) { return true; }
  }

  const accumulated = emptyEntitySet();
  const divergentSet = emptyEntitySet();

  // Inclusive on both bounds: [0 .. divergent_turn].
  if (dt >= 0) {
    for (let t = 0; t <= dt; t++) {
      const entry = turns[t] !== undefined ? turns[t] : turns[String(t)];
      if (entry !== undefined) collectFromTurnEntry(entry, accumulated);
    }
    const dtEntry = turns[dt] !== undefined ? turns[dt] : turns[String(dt)];
    if (dtEntry !== undefined) collectFromTurnEntry(dtEntry, divergentSet);
  }

  if (initial_team && typeof initial_team === 'object') {
    for (const sideKey of ['p1', 'p2']) {
      const side = initial_team[sideKey];
      if (Array.isArray(side)) {
        for (const m of side) if (admit(m)) collectFromMon(m, accumulated);
      }
    }
    if (Array.isArray(initial_team)) {
      for (const m of initial_team) if (admit(m)) collectFromMon(m, accumulated);
    }
  }

  const bugSink = new Set();
  const flagged = flaggedSubset(accumulated, kb, bugSink);
  // Null sink: the divergent-turn pass must not contribute extra bug ids.
  flaggedSubset(divergentSet, kb, null);

  const accEmpty = isEmpty(accumulated);
  const overlap = !accEmpty && divergentOverlapsFlagged(divergentSet, flagged);
  const forAll = !accEmpty && forAllFlagged(accumulated, flagged);

  const bug_ids = Array.from(bugSink);

  if (accEmpty) {
    return { level: CLASSIFICATIONS.NONE, bug_ids: [] };
  }
  if (forAll && overlap) {
    return { level: CLASSIFICATIONS.HIGH, bug_ids };
  }
  if (overlap) {
    return { level: CLASSIFICATIONS.LOW, bug_ids };
  }
  return { level: CLASSIFICATIONS.NONE, bug_ids: [] };
}

module.exports = {
  classify,
  loadKnownBugs,
  collectFromMon,
  GUARDED_ITEM_IDS,
  GUARDED_MOVE_IDS,
};
