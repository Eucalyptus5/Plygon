'use strict';

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { spawnSync } = require('child_process');

const { ACTION_STRUGGLE } = require('./constants.js');

const ID_MAPS_DIR = path.join(__dirname, '..', 'id_maps');
const HARNESS_DIR = path.join(__dirname, '..', 'harness');
const RUN_SCENARIO_BIN = path.join(
  HARNESS_DIR, 'run_scenario', 'target', 'debug', 'run_scenario'
);
const { showdownSim } = require('../harness/lib/showdown_dir.js');

const SPECIES_MAP_PATH = path.join(ID_MAPS_DIR, 'species_map.json');
const ITEM_MAP_PATH    = path.join(ID_MAPS_DIR, 'item_map.json');
const MOVE_MAP_PATH    = path.join(ID_MAPS_DIR, 'move_map.json');
const ABILITY_MAP_PATH = path.join(ID_MAPS_DIR, 'ability_map.json');
const FORME_REVERSE_MAP_PATH = path.join(ID_MAPS_DIR, 'engine_forme_to_showdown.json');

// State is an external object so extendScenario keeps drawing from turn 1's PRNG sequence.
function makeRngState(seed) {
  return { state: seed >>> 0 };
}

function rngStep(s) {
  s.state = (s.state + 0x6D2B79F5) >>> 0;
  let t = s.state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

function mulberry32(seed) {
  const st = makeRngState(seed);
  return () => rngStep(st);
}

function randInt(rng, lo, hi) {
  return lo + Math.floor(rng() * (hi - lo + 1));
}

function pickUniform(rng, arr) {
  return arr[Math.floor(rng() * arr.length)];
}

function shuffleInPlace(rng, arr) {
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    const t = arr[i]; arr[i] = arr[j]; arr[j] = t;
  }
  return arr;
}

function stableSortedDistinct(map) {
  return Array.from(new Set(Object.values(map))).sort((a, b) => a - b);
}

const SPECIES_SKIP_IDS = new Set([
  'zorua', 'zoroark', 'zoruahisui', 'zoroarkhisui',
]);

const FORM_CHANGE_ITEM_IDS = new Set([
  'redorb', 'blueorb', 'griseousorb', 'rustedsword', 'rustedshield',
  'wellspringmask', 'hearthflamemask', 'cornerstonemask',
]);

const ABILITY_SKIP_IDS = new Set([
]);

const ILLEGAL_TIERS = new Set(['Illegal', 'Unreleased', 'CAP', 'CAP NFE', 'CAP LC']);

let _ctx = null;

function getContext() {
  if (_ctx) return _ctx;

  const speciesMap  = JSON.parse(fs.readFileSync(SPECIES_MAP_PATH,  'utf-8'));
  const itemMap     = JSON.parse(fs.readFileSync(ITEM_MAP_PATH,     'utf-8'));
  const moveMap     = JSON.parse(fs.readFileSync(MOVE_MAP_PATH,     'utf-8'));
  const abilityMap  = JSON.parse(fs.readFileSync(ABILITY_MAP_PATH,  'utf-8'));

  let formeReverseMap = {};
  if (fs.existsSync(FORME_REVERSE_MAP_PATH)) {
    formeReverseMap = JSON.parse(fs.readFileSync(FORME_REVERSE_MAP_PATH, 'utf-8'));
  }

  const Sim = require(showdownSim());
  const dex = Sim.Dex.mod('gen9');

  const speciesById = {};
  for (const s of dex.species.all()) {
    if (s.num > 0) {
      if (!speciesById[s.num] || !s.forme) speciesById[s.num] = s;
    }
  }

  const nameById = {};
  for (const [showdownName, engineId] of Object.entries(speciesMap)) {
    if (!nameById[engineId]) nameById[engineId] = showdownName;
  }

  function reverseLookupSpecies(engineId) {
    const fr = formeReverseMap[String(engineId)];
    if (fr && fr.showdown_forme_name) {
      const s = dex.species.get(fr.showdown_forme_name);
      if (s && s.exists) return s;
    }
    if (engineId <= 1025 && speciesById[engineId]) return speciesById[engineId];
    const nm = nameById[engineId];
    if (nm) {
      const s = dex.species.get(nm);
      if (s && s.exists) return s;
    }
    return null;
  }

  function meetsGen9Legality(s) {
    if (!s) return false;
    if (!(s.num > 0)) return false;
    if (s.isNonstandard) return false;
    if (s.gen > 9) return false;
    if (s.tier && ILLEGAL_TIERS.has(s.tier)) return false;
    if (s.forme === 'Gmax') return false;
    if (SPECIES_SKIP_IDS.has(s.id)) return false;
    return true;
  }

  const rawSpeciesDomain = stableSortedDistinct(speciesMap);
  const speciesDomain = [];
  for (const id of rawSpeciesDomain) {
    const s = reverseLookupSpecies(id);
    if (!s) continue;
    if (!meetsGen9Legality(s)) continue;
    // An Ogerpon forme is only addressable when the forme reverse map names it.
    if (s.id && s.id.startsWith('ogerpon') && !formeReverseMap[String(id)]) continue;
    speciesDomain.push(id);
  }

  if (speciesDomain.length < 500) {
    console.error(
      `generate.js: species domain size ${speciesDomain.length} < 500 floor — failing loud.`
    );
    process.exit(1);
  }

  const rawItemDomain = stableSortedDistinct(itemMap);
  const itemNameById = {};
  for (const [name, id] of Object.entries(itemMap)) {
    if (!itemNameById[id]) itemNameById[id] = name;
  }
  const itemDomain = [0];
  for (const id of rawItemDomain) {
    if (id === 0) continue;
    const nm = itemNameById[id];
    if (nm && FORM_CHANGE_ITEM_IDS.has(nm)) continue;
    itemDomain.push(id);
  }

  if (itemDomain.length === 0) {
    console.error('generate.js: empty item pool — failing loud.'); process.exit(1);
  }
  if (Object.keys(moveMap).length === 0) {
    console.error('generate.js: empty move map — failing loud.'); process.exit(1);
  }
  if (Object.keys(abilityMap).length === 0) {
    console.error('generate.js: empty ability map — failing loud.'); process.exit(1);
  }

  _ctx = {
    speciesMap, itemMap, moveMap, abilityMap, formeReverseMap,
    Sim, dex,
    speciesById, nameById,
    reverseLookupSpecies,
    speciesDomain, itemDomain,
  };
  return _ctx;
}

function getDomainStats() {
  const c = getContext();
  return {
    species_domain_size: c.speciesDomain.length,
    item_domain_size: c.itemDomain.length,
    move_map_size: Object.keys(c.moveMap).length,
    ability_map_size: Object.keys(c.abilityMap).length,
  };
}

function sampleAbility(rng, dex, species, abilityMap) {
  const abilitiesObj = species.abilities || {};
  const keys = Object.keys(abilitiesObj).filter(k => abilitiesObj[k]);
  if (keys.length === 0) return null;
  if (keys.length === 1 && keys[0] === '0') {
    const id = abilityMap[toID(abilitiesObj['0'])];
    return id || null;
  }

  for (let attempt = 0; attempt < 8; attempt++) {
    const k = pickUniform(rng, keys);
    const name = abilitiesObj[k];
    if (!name) continue;
    if (k === 'S') {
      // A signature ability is listed only on the forme that owns it, so accept it.
    }
    const idKey = toID(name);
    if (ABILITY_SKIP_IDS.has(idKey)) continue;
    const aid = abilityMap[idKey];
    if (typeof aid === 'number') return aid;
  }
  const fallbackName = abilitiesObj['0'];
  if (fallbackName) {
    const id = abilityMap[toID(fallbackName)];
    if (typeof id === 'number') return id;
  }
  return null;
}

function toID(s) {
  return ('' + s).toLowerCase().replace(/[^a-z0-9]/g, '');
}

function gen9MoveIds(dex, species, moveMap) {
  const moveIdSet = new Set();

  const ld = dex.species.getLearnsetData(species.id);
  if (ld && ld.learnset) {
    for (const [moveid, sources] of Object.entries(ld.learnset)) {
      if (sources && sources.some(t => typeof t === 'string' && t[0] === '9')) {
        moveIdSet.add(moveid);
      }
    }
  } else {
    let full;
    try { full = dex.species.getFullLearnset(species.id); } catch (_) { full = []; }
    if (full && full.length > 0) {
      for (const entry of full) {
        if (!entry || !entry.learnset) continue;
        for (const [moveid, sources] of Object.entries(entry.learnset)) {
          if (sources && sources.some(t => typeof t === 'string' && t[0] === '9')) {
            moveIdSet.add(moveid);
          }
        }
      }
    }
  }

  const ids = [];
  for (const m of moveIdSet) {
    const eid = moveMap[m];
    if (typeof eid === 'number' && eid > 0) ids.push(eid);
  }
  return ids;
}

function sampleMoves(rng, dex, species, moveMap) {
  const pool = gen9MoveIds(dex, species, moveMap);
  if (pool.length === 0) return null;
  const want = randInt(rng, 1, Math.min(4, pool.length));
  const copy = pool.slice();
  shuffleInPlace(rng, copy);
  const picks = copy.slice(0, want);
  while (picks.length < 4) picks.push(0);
  return picks;
}

function sampleEvs(rng) {
  const evs = [0, 0, 0, 0, 0, 0];
  let remaining = 510;
  const order = [0, 1, 2, 3, 4, 5];
  shuffleInPlace(rng, order);
  for (const idx of order) {
    if (remaining <= 0) break;
    const cap = Math.min(252, remaining);
    const v = randInt(rng, 0, cap);
    evs[idx] = v;
    remaining -= v;
  }
  return evs;
}

function buildSide(rng, ctx, opts) {
  const {
    teamSize, oppTakenSpecies, banOpposingSame,
  } = opts;

  const taken = new Set();
  const team = [];

  let attempts = 0;
  while (team.length < teamSize && attempts < 256) {
    attempts++;
    const sid = pickUniform(rng, ctx.speciesDomain);
    if (taken.has(sid)) continue;
    if (banOpposingSame && oppTakenSpecies.has(sid)) continue;
    const species = ctx.reverseLookupSpecies(sid);
    if (!species) continue;

    const moves = sampleMoves(rng, ctx.dex, species, ctx.moveMap);
    if (!moves) continue;

    const aid = sampleAbility(rng, ctx.dex, species, ctx.abilityMap);
    if (typeof aid !== 'number') continue;

    const itemId = pickUniform(rng, ctx.itemDomain);

    const mon = {
      species_id: sid,
      ability_id: aid,
      item_id: itemId,
      moves,
      ivs: [31, 31, 31, 31, 31, 31],
      evs: sampleEvs(rng),
      nature: randInt(rng, 0, 24),
      level: randInt(rng, 1, 100),
      tera_type: randInt(rng, 0, 17),
      is_female: rng() < 0.5,
    };
    team.push(mon);
    taken.add(sid);
  }

  if (team.length === 0) return null;
  return team;
}

// Struggle is emitted only when the engine's probe lists it; showdown_runner cannot replay an unforced Struggle.
function pickActionByte(rng, legalBytes) {
  if (legalBytes && legalBytes.includes(ACTION_STRUGGLE)) return ACTION_STRUGGLE;
  const inRange = (legalBytes || []).filter(b => b >= 0 && b <= 10);
  if (inRange.length === 0) {
    throw new Error('pickActionByte: empty legal-action probe (no 0-10 byte and no engine-sanctioned 255)');
  }
  return pickUniform(rng, inRange);
}

async function turn1LegalActions(teams, enginePool) {
  const probe = {
    name: 'fuzz_probe',
    mode: 'legal_actions',
    teams,
  };
  let r;
  if (enginePool) {
    try {
      r = await enginePool.runScenario(probe, 30000);
    } catch (e) {
      throw new Error(
        `turn-1 legal_actions probe failed (server mode): reason=${e.reason || 'unknown'} stderr=${(e.stderr || '').slice(0, 512)}`
      );
    }
  } else {
    r = spawnSync(RUN_SCENARIO_BIN, [], {
      input: JSON.stringify(probe),
      encoding: 'utf-8',
      maxBuffer: 64 * 1024 * 1024,
    });
  }
  if (r.status !== 0) {
    throw new Error(
      `turn-1 legal_actions probe failed: status=${r.status} stderr=${r.stderr}`
    );
  }
  let parsed;
  try { parsed = JSON.parse(r.stdout); }
  catch (e) {
    throw new Error(`turn-1 legal_actions probe: invalid JSON: ${e.message}`);
  }
  if (!parsed.success) {
    throw new Error(`turn-1 legal_actions probe: success=false error=${parsed.error}`);
  }
  if (!parsed.both_sides) {
    throw new Error('turn-1 legal_actions probe: response missing both_sides');
  }
  const p1 = parsed.both_sides.p1 || [];
  const p2 = parsed.both_sides.p2 || [];
  if (p1.length === 0 || p2.length === 0) {
    throw new Error(
      `turn-1 legal_actions probe: empty bytes (p1=${p1.length} p2=${p2.length}); turn 1 must be PHASE_ACTIONS`
    );
  }
  return { p1, p2 };
}

function snapshotMon(mon) {
  if (!mon) return null;
  return {
    species_id: mon.species_id,
    ability_id: mon.ability_id,
    item_id: mon.item_id,
    move_ids: Array.isArray(mon.moves) ? mon.moves.slice() : [],
  };
}

// entity_sets_by_turn[T] is the mon active at the start of turn T, so turn i's state_after keys T=i+1.
function extractEntitySets(turnResults) {
  const out = {};
  if (!Array.isArray(turnResults)) return out;
  for (let i = 0; i < turnResults.length; i++) {
    const tr = turnResults[i];
    const sa = tr && tr.state_after;
    if (!sa || Array.isArray(sa)) continue;
    const p1 = sa.p1 || {};
    const p2 = sa.p2 || {};
    const p1Mon = (p1.team || [])[p1.active_index];
    const p2Mon = (p2.team || [])[p2.active_index];
    const nextTurnIndex = i + 1;
    out[nextTurnIndex] = {
      p1: snapshotMon(p1Mon),
      p2: snapshotMon(p2Mon),
    };
  }
  return out;
}

function initialTeamFromTeams(teams) {
  return {
    p1: teams.p1.map(m => ({
      species_id: m.species_id,
      ability_id: m.ability_id,
      item_id: m.item_id,
      move_ids: m.moves.slice(),
    })),
    p2: teams.p2.map(m => ({
      species_id: m.species_id,
      ability_id: m.ability_id,
      item_id: m.item_id,
      move_ids: m.moves.slice(),
    })),
  };
}

// At most one turn per scenario runs all_rolls; its index is drawn once, at scenario birth.
function decideRngModePlan(rng, maxTurns, rngModeSplit) {
  const forceAllProb = (rngModeSplit && typeof rngModeSplit.force_all === 'number')
    ? rngModeSplit.force_all : 0.9;
  if (rng() < forceAllProb) {
    return { rng_mode_used: 'force_all', allRollsTurnIdx: -1 };
  }
  const allRollsTurnIdx = randInt(rng, 0, Math.max(0, maxTurns - 1));
  return { rng_mode_used: 'all_rolls', allRollsTurnIdx };
}

function rngModeForTurn(plan, turnIdx) {
  return (plan.allRollsTurnIdx === turnIdx) ? 'all_rolls' : 'force_all';
}

// The returned scenario holds only turn 1; the driver appends the rest via extendScenario.
async function generateScenario(seed, config, enginePool) {
  const cfg = config || {};
  const rngState = makeRngState(seed >>> 0);
  const rng = () => rngStep(rngState);
  const ctx = getContext();

  const banOpposingSame = !!cfg.ban_opposing_same_species;
  const maxTurns = (typeof cfg.max_turns_per_scenario === 'number')
    ? cfg.max_turns_per_scenario : 8;

  const p1Size = randInt(rng, 1, 6);
  const p2Size = randInt(rng, 1, 6);

  let p1Team = null, p2Team = null;
  for (let attempt = 0; attempt < 8 && !p1Team; attempt++) {
    p1Team = buildSide(rng, ctx, {
      teamSize: p1Size,
      oppTakenSpecies: new Set(),
      banOpposingSame: false,
    });
  }
  if (!p1Team) throw new Error('generate: failed to build p1 side after 8 attempts');

  const p1Species = new Set(p1Team.map(m => m.species_id));
  for (let attempt = 0; attempt < 8 && !p2Team; attempt++) {
    p2Team = buildSide(rng, ctx, {
      teamSize: p2Size,
      oppTakenSpecies: p1Species,
      banOpposingSame,
    });
  }
  if (!p2Team) throw new Error('generate: failed to build p2 side after 8 attempts');

  const teams = { p1: p1Team, p2: p2Team };

  const t1 = await turn1LegalActions(teams, enginePool);
  const turn1 = {
    p1_action: pickActionByte(rng, t1.p1),
    p2_action: pickActionByte(rng, t1.p2),
  };
  const rngPlan = decideRngModePlan(rng, maxTurns, cfg.rng_mode_split);
  turn1.rng_mode = rngModeForTurn(rngPlan, 0);

  // extendScenario reads these off rngState to stamp rng_mode and stop at the turn bound.
  rngState.rngPlan = rngPlan;
  rngState.maxTurns = maxTurns;

  const scenario = {
    name: `fuzz_seed_${seed}`,
    mode: 'execute_turns',
    teams,
    turns: [turn1],
  };

  const initial_team = initialTeamFromTeams(teams);

  // Turn 0 has no preceding turn result, so its entity set comes from each side's lead.
  const entity_sets_by_turn = {
    0: {
      p1: snapshotMon(teams.p1[0]),
      p2: snapshotMon(teams.p2[0]),
    },
  };

  return {
    scenario,
    entity_sets_by_turn,
    initial_team,
    rng_mode_used: rngPlan.rng_mode_used,
    rngState,
  };
}

function extendScenario(scenario, rngState, legalBytesP1, legalBytesP2) {
  if (!scenario || !Array.isArray(scenario.turns)) {
    throw new Error('extendScenario: scenario.turns missing');
  }
  if (!rngState || rngState.rngPlan === undefined) {
    throw new Error('extendScenario: rngState missing rngPlan; was scenario produced by generateScenario?');
  }
  const turnIdx = scenario.turns.length;
  if (typeof rngState.maxTurns === 'number' && turnIdx >= rngState.maxTurns) {
    return false;
  }
  const rng = () => rngStep(rngState);
  const p1 = pickActionByte(rng, legalBytesP1);
  const p2 = pickActionByte(rng, legalBytesP2);
  scenario.turns.push({
    p1_action: p1,
    p2_action: p2,
    rng_mode: rngModeForTurn(rngState.rngPlan, turnIdx),
  });
  return true;
}

function stableStringify(obj) {
  if (obj === null || typeof obj !== 'object') return JSON.stringify(obj);
  if (Array.isArray(obj)) return '[' + obj.map(stableStringify).join(',') + ']';
  const keys = Object.keys(obj).sort();
  return '{' + keys.map(k => JSON.stringify(k) + ':' + stableStringify(obj[k])).join(',') + '}';
}

async function selfTest() {
  const seed = 0;
  const config = { max_turns_per_scenario: 4, rng_mode_split: { force_all: 0.9 } };
  const { SubprocessPool } = require('./lib/subprocess_pool.js');
  const enginePool = new SubprocessPool({
    command: RUN_SCENARIO_BIN,
    args: ['--server-mode'],
    label: 'engine',
    kind: 'engine',
    recycleAfter: Infinity,
  });
  let exitCode = 0;
  try {
    const hashes = [];
    for (let i = 0; i < 3; i++) {
      const r = await generateScenario(seed, config, enginePool);
      const s = stableStringify(r.scenario);
      hashes.push(crypto.createHash('sha256').update(s).digest('hex'));
    }
    const ok = hashes[0] === hashes[1] && hashes[1] === hashes[2];
    if (ok) {
      console.log(`PASS determinism self-test: ${hashes[0]}`);
    } else {
      console.error(`FAIL determinism self-test: ${hashes.join(' ')}`);
      exitCode = 1;
    }
  } finally {
    await enginePool.shutdown();
  }
  process.exit(exitCode);
}

if (require.main === module) {
  if (process.argv.includes('--self-test')) {
    selfTest().catch((e) => {
      console.error(`generate.js: self-test failed: ${e.stack || e.message}`);
      process.exit(1);
    });
  } else if (process.argv.includes('--domain-stats')) {
    console.log(JSON.stringify(getDomainStats()));
  } else {
    console.error('generate.js: invoke with --self-test or --domain-stats, or require() it.');
    process.exit(2);
  }
}

module.exports = {
  generateScenario,
  extendScenario,
  extractEntitySets,
  getDomainStats,
  pickActionByte,
  stableSortedDistinct,
  mulberry32,
  makeRngState,
  rngStep,
};
