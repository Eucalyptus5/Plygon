#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const {Battle, Teams, Dex} = require(require('./lib/showdown_dir.js').showdownSim());
const {ACTION_STRUGGLE} = require('../fuzzer/constants.js');

const dex = Dex.mod('gen9');

const TYPE_NAMES = [
  'Normal', 'Fighting', 'Flying', 'Poison', 'Ground', 'Rock',
  'Bug', 'Ghost', 'Steel', 'Fire', 'Water', 'Grass',
  'Electric', 'Psychic', 'Ice', 'Dragon', 'Dark', 'Fairy',
];

const FORME_MAP_PATH = path.join(__dirname, '..', 'id_maps', 'engine_forme_to_showdown.json');
let engineFormeToShowdown = {};
if (fs.existsSync(FORME_MAP_PATH)) {
  const raw = fs.readFileSync(FORME_MAP_PATH, 'utf-8');
  try {
    engineFormeToShowdown = JSON.parse(raw);
  } catch (e) {
    console.error(`showdown_runner.js: failed to parse ${FORME_MAP_PATH}: ${e.message}`);
    process.exit(1);
  }
}

const engineIdToShowdownSpecies = {};
for (const [engineIdStr, entry] of Object.entries(engineFormeToShowdown)) {
  const sp = dex.species.get(entry.showdown_forme_name);
  if (sp && sp.exists) engineIdToShowdownSpecies[engineIdStr] = sp;
}

const moveById = {};
for (const m of dex.moves.all()) {
  if (m.num > 0) moveById[m.num] = m;
}

const speciesById = {};
for (const s of dex.species.all()) {
  if (s.num > 0) {
    if (!speciesById[s.num] || !s.forme) {
      speciesById[s.num] = s;
    }
  }
}
const speciesByIdAll = {};
for (const s of dex.species.all()) {
  if (s.num > 0) {
    if (!speciesByIdAll[s.num]) speciesByIdAll[s.num] = [];
    speciesByIdAll[s.num].push(s);
  }
}

const abilityById = {};
for (const a of dex.abilities.all()) {
  if (a.num > 0) abilityById[a.num] = a;
}

// The engine uses Showdown's spritenum as its item ID.
const itemBySpritenum = {};
for (const i of dex.items.all()) {
  if (i.spritenum > 0) {
    if (!itemBySpritenum[i.spritenum] || i.isNonstandard !== 'Past') {
      itemBySpritenum[i.spritenum] = i;
    }
  }
}

const STAT_KEYS = [null, 'atk', 'def', 'spa', 'spd', 'spe'];
const allNatures = [...dex.natures.all()];

function engineNatureToShowdown(n) {
  const boosted_idx = Math.floor(n / 5) + 1;
  const reduced_idx = (n % 5) + 1;
  if (boosted_idx === reduced_idx) {
    return 'Hardy';
  }
  const boosted = STAT_KEYS[boosted_idx];
  const reduced = STAT_KEYS[reduced_idx];
  const match = allNatures.find(nat => nat.plus === boosted && nat.minus === reduced);
  return match ? match.name : 'Hardy';
}

function buildTeam(mons) {
  return mons.map((m, i) => {
    const species =
        engineIdToShowdownSpecies[String(m.species_id)]
     ?? speciesById[m.species_id];
    if (!species) throw new Error(`Unknown species_id: ${m.species_id}`);

    const ability = abilityById[m.ability_id];
    if (!ability) throw new Error(`Unknown ability_id: ${m.ability_id}`);

    const item = m.item_id ? itemBySpritenum[m.item_id] : null;

    const moves = m.moves.filter(id => id > 0).map(id => {
      const move = moveById[id];
      if (!move) throw new Error(`Unknown move_id: ${id}`);
      return move.name;
    });

    const evs = {
      hp: (m.evs && m.evs[0]) || 0,
      atk: (m.evs && m.evs[1]) || 0,
      def: (m.evs && m.evs[2]) || 0,
      spa: (m.evs && m.evs[3]) || 0,
      spd: (m.evs && m.evs[4]) || 0,
      spe: (m.evs && m.evs[5]) || 0,
    };

    const ivs = {
      hp: (m.ivs && m.ivs[0] !== undefined) ? m.ivs[0] : 31,
      atk: (m.ivs && m.ivs[1] !== undefined) ? m.ivs[1] : 31,
      def: (m.ivs && m.ivs[2] !== undefined) ? m.ivs[2] : 31,
      spa: (m.ivs && m.ivs[3] !== undefined) ? m.ivs[3] : 31,
      spd: (m.ivs && m.ivs[4] !== undefined) ? m.ivs[4] : 31,
      spe: (m.ivs && m.ivs[5] !== undefined) ? m.ivs[5] : 31,
    };

    const nature = engineNatureToShowdown(m.nature || 0);
    const teraType = TYPE_NAMES[m.tera_type || 0] || 'Normal';

    const prefixedName = `${i}_${species.name}`;
    if (prefixedName.includes('|') || prefixedName.includes(']')) {
      throw new Error(`buildTeam: prefixed name "${prefixedName}" contains a Teams.pack delimiter (| or ]); slot=${i} species=${species.name}`);
    }

    return {
      name: prefixedName,
      species: species.name,
      item: item ? item.name : '',
      ability: ability.name,
      moves,
      nature,
      evs,
      ivs,
      level: m.level || 100,
      gender: m.is_female ? 'F' : 'M',
      teraType,
    };
  });
}

function extractSnapshot(battle) {
  return {
    phase: battle.ended ? 'game_over' : 'actions',
    field: {
      turn: battle.turn,
      weather: battle.field.weather || 'none',
      weather_turns: battle.field.weatherState?.duration || 0,
      terrain: battle.field.terrain || 'none',
      terrain_turns: battle.field.terrainState?.duration || 0,
      trick_room_turns: battle.field.getPseudoWeather('trickroom')?.duration || 0,
      gravity_turns: battle.field.getPseudoWeather('gravity')?.duration || 0,
    },
    p1: extractSide(battle, battle.p1),
    p2: extractSide(battle, battle.p2),
  };
}

function extractSide(battle, side) {
  const active = side.active[0];
  const team = side.pokemon.map((mon, i) => {
    const m = /^(\d+)_/.exec(mon.name);
    const parsed = m ? parseInt(m[1], 10) : NaN;
    return {
      current_array_slot: i,
      build_slot: Number.isFinite(parsed) ? parsed : null,
      species: mon.species.name,
      species_id: mon.baseSpecies?.num ?? mon.species.num,
      ability: mon.ability,
      ability_id: dex.abilities.get(mon.ability)?.num || 0,
      item: mon.item || 'none',
      item_spritenum: mon.item ? (dex.items.get(mon.item)?.spritenum || 0) : 0,
      current_hp: mon.hp,
      max_hp: mon.maxhp,
      status: mon.status || 'none',
      status_counter: mon.statusState?.stage || 0,
      is_fainted: mon.fainted,
      tera_type: mon.teraType ? TYPE_NAMES.indexOf(mon.teraType) : 0,
      is_terastallized: !!mon.terastallized,
      moves: mon.moves,
      pp: mon.moves.map(mv => mon.getMoveData(dex.moves.get(mv))?.pp || 0),
      stats: {
        atk: mon.storedStats?.atk || 0,
        def: mon.storedStats?.def || 0,
        spa: mon.storedStats?.spa || 0,
        spd: mon.storedStats?.spd || 0,
        spe: mon.storedStats?.spe || 0,
      },
    };
  });

  const activeData = active ? {
    boosts: {...(active.boosts || {atk:0,def:0,spa:0,spd:0,spe:0,accuracy:0,evasion:0})},
    volatiles: Object.keys(active.volatiles || {}),
    substitute_hp: active.volatiles?.substitute?.hp || 0,
    confusion_turns: active.volatiles?.confusion?.time || 0,
    taunt_turns: active.volatiles?.taunt?.duration || 0,
    encore_turns: active.volatiles?.encore?.duration || 0,
    types: [...(active.types || [])],
    is_terastallized: active.terastallized || false,
    effective_ability_id: active.ignoringAbility() ? 0 : (dex.abilities.get(active.ability)?.num || 0),
    effective_item_id: active.item ? (dex.items.get(active.item)?.spritenum || 0) : 0,
    effective_species_id: active.species?.num || 0,
  } : null;

  const sideConditions = {};
  for (const [key, val] of Object.entries(side.sideConditions || {})) {
    sideConditions[key] = {
      layers: val.layers || 1,
      duration: val.duration || 0,
    };
  }

  return {
    active_index: 0,
    team,
    active: activeData,
    side_conditions: sideConditions,
  };
}

function applyStateOverrides(battle, overrides) {
  if (overrides.field) {
    const f = overrides.field;
    if (f.weather !== undefined && f.weather !== 0) {
      const weatherNames = {1: 'sunnyday', 2: 'raindance', 3: 'sandstorm', 4: 'snowscape'};
      const wName = weatherNames[f.weather];
      if (wName) battle.field.setWeather(wName, battle.p1.active[0]);
    }
    if (f.terrain !== undefined && f.terrain !== 0) {
      const terrainNames = {1: 'electricterrain', 2: 'grassyterrain', 3: 'psychicterrain', 4: 'mistyterrain'};
      const tName = terrainNames[f.terrain];
      if (tName) battle.field.setTerrain(tName);
    }
    if (f.trick_room_turns) battle.field.addPseudoWeather('trickroom', battle.p1.active[0]);
    if (f.gravity_turns) battle.field.addPseudoWeather('gravity', battle.p1.active[0]);
  }

  const applySideOverrides = (side, sideOv) => {
    if (!sideOv) return;
    const active = side.active[0];
    if (sideOv.active_overrides && active) {
      const ao = sideOv.active_overrides;
      if (ao.boosts) {
        const statKeys = ['atk', 'def', 'spa', 'spd', 'spe', 'accuracy', 'evasion'];
        for (let i = 0; i < ao.boosts.length && i < statKeys.length; i++) {
          active.boosts[statKeys[i]] = ao.boosts[i];
        }
      }
      if (ao.status !== undefined && ao.status !== 0) {
        const statusNames = {1: 'brn', 2: 'par', 3: 'psn', 4: 'tox', 5: 'slp', 6: 'frz'};
        const sName = statusNames[ao.status];
        if (sName) active.setStatus(sName);
      }
      if (ao.current_hp !== undefined) {
        active.hp = ao.current_hp;
        active.sethp(ao.current_hp);
      }
      if (ao.attracted) {
        // The attract volatile needs a source, so infatuate toward the opposing active.
        const opp = side === battle.p1 ? battle.p2 : battle.p1;
        if (opp && opp.active[0]) active.addVolatile('attract', opp.active[0]);
      }
    }
    if (sideOv.side_conditions) {
      const sc = sideOv.side_conditions;
      if (sc.reflect_turns) side.addSideCondition('reflect');
      if (sc.light_screen_turns) side.addSideCondition('lightscreen');
      if (sc.aurora_veil_turns) side.addSideCondition('auroraveil');
      if (sc.stealth_rock) side.addSideCondition('stealthrock');
      if (sc.spikes) for (let i = 0; i < sc.spikes; i++) side.addSideCondition('spikes');
      if (sc.toxic_spikes) for (let i = 0; i < sc.toxic_spikes; i++) side.addSideCondition('toxicspikes');
      if (sc.sticky_web) side.addSideCondition('stickyweb');
    }
  };

  if (overrides.p1) applySideOverrides(battle.p1, overrides.p1);
  if (overrides.p2) applySideOverrides(battle.p2, overrides.p2);
}

// Byte 255 is legitimate only when Struggle or recharge is the side's single forced action.
function struggleForced(side) {
  const active = side && side.active && side.active[0];
  if (!active || active.fainted) return false;
  const req = active.getMoveRequestData();
  const moves = req && req.moves;
  if (!Array.isArray(moves) || moves.length !== 1) return false;
  const id = moves[0].id;
  return id === 'struggle' || id === 'recharge';
}

// Downstream consumers key on this exact string.
const CONTRACT_STRUGGLE255 = 'contract:struggle255_not_forced';

class ContractViolation extends Error {
  constructor(reason) { super(reason); this.contractReason = reason; }
}

function actionToChoice(action, battle, side) {
  if (action === ACTION_STRUGGLE) {
    if (!struggleForced(side)) throw new ContractViolation(CONTRACT_STRUGGLE255);
    // Struggle or Recharge is then the sole request move.
    return 'move 1';
  }
  if (action <= 3) {
    // Showdown collapses a locked request to one entry, so any slot byte would be out of range.
    const active = side && side.active && side.active[0];
    if (active && !active.fainted) {
      const req = active.getMoveRequestData();
      const moves = req && req.moves;
      if (Array.isArray(moves) && moves.length === 1) return 'move 1';
    }
    return `move ${action + 1}`;
  }
  if (action >= 4 && action <= 9) {
    // Showdown reorders side.pokemon after a switch, so recover the engine slot from the name prefix.
    const k = action - 4;
    if (side && side.pokemon) {
      for (let j = 0; j < side.pokemon.length; j++) {
        const mon = side.pokemon[j];
        const candidates = [];
        if (mon && typeof mon.name === 'string') candidates.push(mon.name);
        if (mon && mon.set && typeof mon.set.name === 'string') candidates.push(mon.set.name);
        for (const nm of candidates) {
          const m = /^(\d+)_/.exec(nm);
          if (m) {
            const i = parseInt(m[1], 10);
            if (Number.isFinite(i) && i === k) {
              return `switch ${j + 1}`;
            }
            break; // parsed a prefix that didn't match; this mon is not slot k
          }
        }
      }
    }
    // Illusion or Transform can substitute the display name; the naive slot is the intended fallback.
    return `switch ${k + 1}`;
  }
  if (action === 10) {
    return 'move 1 terastallize';
  }
  return 'default';
}

// gen9customgame does not load the endless-battle clause, so the turn-1000 tie is the only ceiling.
const SELF_DRIVE_CAP = 1000;

function isNotAllChoicesDone(e) {
  return !!e && typeof e.message === 'string' && e.message.includes('Not all choices done');
}

// commitChoices clears the queue before it throws, so capture the pre-clear length at entry.
function installRejectQueueProbe(battle) {
  const original = battle.commitChoices.bind(battle);
  battle.commitChoices = function (...args) {
    const lenAtEntry = (this.queue && Array.isArray(this.queue.list)) ? this.queue.list.length : 0;
    const choicesAtEntry = (this.queue && Array.isArray(this.queue.list))
      ? this.queue.list.map((a) => (a && a.choice) || null) : [];
    try {
      return original(...args);
    } catch (e) {
      if (isNotAllChoicesDone(e)) {
        this._rejectOldQueueLen = lenAtEntry;
        this._rejectOldQueueChoices = choicesAtEntry;
      }
      throw e;
    }
  };
}

// battle.winner is a side NAME, not 'p1'/'p2': '' on a tie, undefined while ongoing.
function mapWinner(battle) {
  const w = battle.winner;
  if (w === undefined || w === null) return null; // ongoing
  if (w === '') return 'tie';
  const names = battle.sides.map((s) => s.name);
  const idx = names.indexOf(w);
  if (idx === -1) {
    throw new Error(`mapWinner: battle.winner=${JSON.stringify(w)} not in side names ${JSON.stringify(names)} — winner mapping would invert silently`);
  }
  return idx === 0 ? 'p1' : (idx === 1 ? 'p2' : `side${idx}`);
}

// Reject rather than coerce: a comparator computing base+k must not silently mis-seed.
function deriveSeed(seedInput) {
  if (Array.isArray(seedInput)) {
    if (seedInput.length !== 4) throw new Error('seed array must have exactly 4 lanes');
    for (const x of seedInput) {
      if (!Number.isInteger(x) || x < 0 || x > 65535) throw new Error('seed lane out of [0,65535]');
    }
    return seedInput.slice();
  }
  if (typeof seedInput !== 'number' && typeof seedInput !== 'bigint') {
    throw new Error('seed must be a number or a [u16;4] array');
  }
  if (typeof seedInput === 'number' && !Number.isInteger(seedInput)) {
    throw new Error('seed must be an integer');
  }
  const n = BigInt(seedInput);
  if (n < 0n) throw new Error('seed must be non-negative');
  if (n > (1n << 64n) - 1n) throw new Error('seed exceeds u64 range');
  return [
    Number((n >> 48n) & 0xFFFFn),
    Number((n >> 32n) & 0xFFFFn),
    Number((n >> 16n) & 0xFFFFn),
    Number(n & 0xFFFFn),
  ];
}

function buildBattle(scenario) {
  const p1Team = buildTeam(scenario.teams.p1);
  const p2Team = buildTeam(scenario.teams.p2);
  const unforced = scenario.unforced === true;

  const battle = new Battle({
    formatid: 'gen9customgame',
    seed: unforced ? deriveSeed(scenario.seed) : [1, 2, 3, 4],
  });

  battle.setPlayer('p1', {name: 'P1', team: Teams.pack(p1Team)});
  battle.setPlayer('p2', {name: 'P2', team: Teams.pack(p2Team)});
  const rejectResilient = scenario.reject_resilient === true;
  if (rejectResilient) installRejectQueueProbe(battle); // pre-clear queue capture
  battle.makeChoices('default', 'default'); // Initial switch-in

  if (scenario.state_overrides) {
    applyStateOverrides(battle, scenario.state_overrides);
  }
  return {battle, p1Team, p2Team, unforced, rejectResilient};
}

function handleExecuteTurns(scenario) {
  const {battle, p1Team, p2Team, unforced, rejectResilient} = buildBattle(scenario);

  const initialState = extractSnapshot(battle);
  const turns = [];
  let rejectInfo = null;
  let terminalVia = 'scripted';

  for (let i = 0; i < scenario.turns.length; i++) {
    const turn = scenario.turns[i];
    if (battle.ended) {
      turns.push({
        turn_number: i + 1,
        state_after: extractSnapshot(battle),
        skipped: true,
      });
      continue;
    }

    const rngMode = turn.rng_mode || 'force_all';
    // Unforced ignores per-turn rng_mode entirely so the native prng drives everything.
    const isAllRolls = !unforced && rngMode === 'all_rolls';

    if (isAllRolls) {
      const snapshots = [];
      for (let roll = 0; roll < 16; roll++) {
        const b2 = new Battle({formatid: 'gen9customgame', seed: [1, 2, 3, 4]});
        b2.setPlayer('p1', {name: 'P1', team: Teams.pack(p1Team)});
        b2.setPlayer('p2', {name: 'P2', team: Teams.pack(p2Team)});
        b2.makeChoices('default', 'default');

        // The engine clones from overridden state, so apply overrides before replaying.
        if (scenario.state_overrides) {
          applyStateOverrides(b2, scenario.state_overrides);
        }

        for (let j = 0; j < i; j++) {
          const prevTurn = scenario.turns[j];
          setupRng(b2, prevTurn.rng_mode || 'force_all', 15, prevTurn.rng_overrides);
          const p1c = actionToChoice(prevTurn.p1_action, b2, b2.p1);
          const p2c = actionToChoice(prevTurn.p2_action, b2, b2.p2);
          b2.makeChoices(p1c, p2c);
        }

        setupRng(b2, 'force_all', roll);
        const p1c = actionToChoice(turn.p1_action, b2, b2.p1);
        const p2c = actionToChoice(turn.p2_action, b2, b2.p2);
        b2.makeChoices(p1c, p2c);
        snapshots.push(extractSnapshot(b2));
      }

      setupRng(battle, 'force_all', 15);
      const p1c = actionToChoice(turn.p1_action, battle, battle.p1);
      const p2c = actionToChoice(turn.p2_action, battle, battle.p2);
      battle.makeChoices(p1c, p2c);

      turns.push({
        turn_number: i + 1,
        state_after_all_rolls: snapshots,
      });
    } else {
      if (!unforced) {
        const rollValue = (rngMode === 'min_roll') ? 0 : 15;
        setupRng(battle, rngMode, rollValue, turn.rng_overrides);
      }

      let p1c, p2c;
      try {
        p1c = actionToChoice(turn.p1_action, battle, battle.p1);
        p2c = actionToChoice(turn.p2_action, battle, battle.p2);
        battle.makeChoices(p1c, p2c);
      } catch (e) {
        // A non-forced byte 255 is a replay-contract violation, not an engine divergence.
        const isStruggle255 = e instanceof ContractViolation && e.contractReason === CONTRACT_STRUGGLE255;
        if (!isStruggle255 && !isNotAllChoicesDone(e)) throw e;
        if (!rejectResilient) throw e;
        rejectInfo = {
          turn_index: i,
          turn_number: i + 1,
          p1_action: turn.p1_action,
          p2_action: turn.p2_action,
          p1_choice: p1c !== undefined ? p1c : null,
          p2_choice: p2c !== undefined ? p2c : null,
          // >0 means a pivot whose rest-of-turn was destroyed; 0 means a single-faint forced switch.
          old_queue_len: (typeof battle._rejectOldQueueLen === 'number') ? battle._rejectOldQueueLen : null,
          old_queue_choices: battle._rejectOldQueueChoices || null,
        };
        if (isStruggle255) rejectInfo.reason = CONTRACT_STRUGGLE255;
        terminalVia = 'self_drive';
        break; // abandon remaining scripted turns; recover via self-drive below
      }

      turns.push({
        turn_number: i + 1,
        state_after: extractSnapshot(battle),
      });
    }
  }

  // choose() clears the stuck partial choice, so default/default resolves from the reject point.
  if (rejectInfo) {
    let guard = SELF_DRIVE_CAP;
    let stalled = false;
    while (!battle.ended && guard-- > 0) {
      try {
        battle.makeChoices('default', 'default');
      } catch (e) {
        if (!isNotAllChoicesDone(e)) throw e;
        stalled = true; // default/default still can't resolve — leave ongoing
        break;
      }
    }
    rejectInfo.self_drive_turns = SELF_DRIVE_CAP - guard;
    rejectInfo.self_drive_stalled = stalled;
    rejectInfo.reached_end = !!battle.ended;
    // Push the recovered terminal so winner reads off turns[last] like every other game.
    turns.push({
      turn_number: turns.length + 1,
      state_after: extractSnapshot(battle),
      self_driven: true,
    });
  }

  const result = {
    name: scenario.name,
    mode: 'execute_turns',
    success: true,
    initial_state: initialState,
    turns,
  };
  // Only in reject-resilient mode: with the flag off the output must stay byte-identical.
  if (rejectResilient) {
    result.winner = mapWinner(battle);
    result.ended = !!battle.ended;
    result.reject_info = rejectInfo;
    result.terminal_via = terminalVia;
  }
  return result;
}

function setupRng(battle, mode, rollValue, overrides) {
  const tr = battle.trunc.bind(battle);
  const CRIT_DENOMS = new Set([24, 8, 2, 1]); // critMult values for critRatio 1-4

  let forceCrit = false;
  let forceSecondaries = true;
  let forceAccuracy = true;
  let damageRoll = (rollValue !== undefined) ? rollValue : 15;

  // Engine and Showdown roll conventions are inverted: rollValue is engine-side, 0=min.
  function engineRollToShowdown(engineRoll) {
    return 15 - engineRoll;
  }

  switch (mode) {
    case 'force_all':
      forceCrit = false;
      forceSecondaries = true;
      forceAccuracy = true;
      damageRoll = engineRollToShowdown(rollValue !== undefined ? rollValue : 15);
      break;

    case 'force_none':
      forceCrit = false;
      forceSecondaries = false;
      forceAccuracy = true;
      damageRoll = engineRollToShowdown(rollValue !== undefined ? rollValue : 15);
      break;

    case 'min_roll':
      damageRoll = engineRollToShowdown(0); // engine min → Showdown roll 15 (85%)
      forceCrit = false;
      forceSecondaries = true;
      forceAccuracy = true;
      break;

    case 'max_roll':
      damageRoll = engineRollToShowdown(15); // engine max → Showdown roll 0 (100%)
      forceCrit = false;
      forceSecondaries = true;
      forceAccuracy = true;
      break;

    case 'specific':
      if (overrides) {
        damageRoll = engineRollToShowdown(overrides.damage_roll !== undefined ? overrides.damage_roll : 15);
        forceCrit = !!overrides.crit;
        forceSecondaries = overrides.secondary_trigger !== false;
        forceAccuracy = overrides.accuracy_hit !== false;
      }
      break;
  }

  battle.forceRandomChance = null; // disable the built-in override
  battle.randomChance = function(numerator, denominator) {
    if (numerator === 1 && CRIT_DENOMS.has(denominator)) {
      return forceCrit;
    }
    // Heuristic: a denominator-100 roll is likely accuracy when the numerator is >= 50, else a chance effect.
    if (denominator === 100) {
      if (numerator >= 50) {
        return forceAccuracy;
      }
      return forceSecondaries;
    }
    // Paralysis full para check: randomChance(1, 4)
    if (numerator === 1 && denominator === 4) {
      return forceSecondaries;
    }
    // Freeze thaw check: randomChance(1, 5)
    if (numerator === 1 && denominator === 5) {
      return forceSecondaries;
    }
    // Consecutive Protect failure: randomChance(1, 3), randomChance(1, 9), randomChance(1, 27)
    if (numerator === 1 && (denominator === 3 || denominator === 9 || denominator === 27)) {
      return forceSecondaries;
    }
    // Every denominator-10 chance, not only the contact-ability ones.
    if (denominator === 10) {
      return forceSecondaries;
    }
    return battle.prng.randomChance(numerator, denominator);
  };

  // Showdown triggers a secondary when secondaryRoll < chance, so 0 always fires.
  const origRandom = battle.random.bind(battle);
  battle.random = function(m, n) {
    if (m === 100 && n === undefined) {
      return forceSecondaries ? 0 : 99;
    }
    // The engine clamps rng(n-m) to 0 then adds the min, so the forced two-arg value is m.
    if (n !== undefined && forceSecondaries) {
      return m;
    }
    // Single-arg selection rolls: the engine clamps rng(m) to 0, so force the index-0 pick.
    if (m !== undefined && n === undefined && forceSecondaries) {
      return 0;
    }
    return origRandom(m, n);
  };

  // Intercept at sample, not prng.random, which is shared with the speed-tie shuffle.
  const origSample = battle.sample.bind(battle);
  battle.sample = function(items) {
    if (forceSecondaries && items && items.length) {
      return items[0];
    }
    return origSample(items);
  };
  const origPrngSample = battle.prng.sample.bind(battle.prng);
  battle.prng.sample = function(items) {
    if (forceSecondaries && items && items.length) {
      return items[0];
    }
    return origPrngSample(items);
  };

  // randomizer reads Showdown convention: damageRoll 0 is max damage, 15 is min.
  battle.randomizer = (baseDamage) => tr(tr(baseDamage * (100 - damageRoll)) / 100);
}

function handleCalcDamage(scenario) {
  const p1Team = buildTeam(scenario.teams.p1);
  const p2Team = buildTeam(scenario.teams.p2);
  const params = scenario.calc_damage_params;

  function runDamageRolls(forceCrit) {
    const rolls = [];
    for (let engineRoll = 0; engineRoll < 16; engineRoll++) {
      const battle = new Battle({formatid: 'gen9customgame', seed: [1, 2, 3, 4]});
      battle.setPlayer('p1', {name: 'P1', team: Teams.pack(p1Team)});
      battle.setPlayer('p2', {name: 'P2', team: Teams.pack(p2Team)});
      battle.makeChoices('default', 'default');

      if (scenario.state_overrides) {
        applyStateOverrides(battle, scenario.state_overrides);
      }

      setupRng(battle, 'specific', engineRoll, {
        damage_roll: engineRoll,
        crit: forceCrit,
        secondary_trigger: true,
        accuracy_hit: true,
      });

      // Pin the full-para (1,4) and freeze-thaw (1,5) rolls so a status cannot skew the damage sample.
      const wrappedRandomChance = battle.randomChance;
      battle.randomChance = function(numerator, denominator) {
        if (numerator === 1 && (denominator === 4 || denominator === 5)) return false;
        return wrappedRandomChance.call(this, numerator, denominator);
      };

      const atkSide = params.atk_side === 0 ? battle.p1 : battle.p2;
      const defSide = params.atk_side === 0 ? battle.p2 : battle.p1;
      const attacker = atkSide.active[0];
      const defender = defSide.active[0];
      const move = moveById[params.move_id];
      if (!move) throw new Error(`Unknown move_id in calc_damage: ${params.move_id}`);

      // Skip EOT residuals so damage = move damage only (not burn/poison/etc.)
      const origFieldEvent = battle.fieldEvent.bind(battle);
      battle.fieldEvent = function(eventid, ...args) {
        if (eventid === 'Residual') return;
        return origFieldEvent(eventid, ...args);
      };

      const hp_before = defender.hp;
      if (params.atk_side === 0) {
        battle.makeChoices(`move ${getMoveSLot(attacker, move)}`, 'move 1');
      } else {
        battle.makeChoices('move 1', `move ${getMoveSLot(attacker, move)}`);
      }
      const hp_after = defender.hp;

      rolls.push({
        roll: engineRoll,
        damage: hp_before - hp_after,
        hp_before,
        hp_after,
        crit: forceCrit,
        defender_status: defender.status || 'none',
      });
    }
    return rolls;
  }

  const allRolls = runDamageRolls(false);
  const critResults = (params.include_crit !== false) ? runDamageRolls(true) : [];

  const damages = allRolls.map(r => r.damage);
  const critDamages = critResults.map(r => r.damage);

  return {
    name: scenario.name,
    mode: 'calc_damage',
    success: true,
    results: {
      all_rolls: allRolls,
      crit_results: critResults,
      min_damage: Math.min(...damages),
      max_damage: Math.max(...damages),
      min_damage_crit: critDamages.length ? Math.min(...critDamages) : 0,
      max_damage_crit: critDamages.length ? Math.max(...critDamages) : 0,
    },
  };
}

function getMoveSLot(pokemon, move) {
  const idx = pokemon.moves.indexOf(move.id);
  if (idx >= 0) return idx + 1;
  for (let i = 0; i < pokemon.moves.length; i++) {
    if (pokemon.moves[i] === move.id) return i + 1;
  }
  return 1; // default to slot 1
}

function runOne(input) {
  let raw;
  try {
    raw = JSON.parse(input);
  } catch (e) {
    return JSON.stringify({__req_id: null, name: 'parse', success: false,
                           error: `JSON parse error: ${e.message}`});
  }
  const reqId = raw.__req_id !== undefined ? raw.__req_id : null;
  const scenario = raw;
  try {
    let result;
    switch (scenario.mode) {
      case 'execute_turns':
        result = handleExecuteTurns(scenario);
        break;
      case 'calc_damage':
        result = handleCalcDamage(scenario);
        break;
      default:
        throw new Error(`Unknown mode: ${scenario.mode}`);
    }
    result.__req_id = reqId;
    return JSON.stringify(result);
  } catch (e) {
    return JSON.stringify({__req_id: reqId, name: scenario.name || 'unknown',
                           success: false, error: e.message});
  }
}

function main() {
  const serverMode = process.argv.includes('--server-mode');
  if (serverMode) {
    const readline = require('readline');
    const rl = readline.createInterface({input: process.stdin, terminal: false});

    const idleTimeoutMs = Number(process.env.SHOWDOWN_IDLE_TIMEOUT_MS || 60000);
    let lastInputMs = Date.now();
    setInterval(() => {
      if (Date.now() - lastInputMs > idleTimeoutMs) process.exit(0);
    }, 1000).unref();

    rl.on('line', (line) => {
      lastInputMs = Date.now();
      // A blank line is a protocol violation; the parse error drives the pool's respawn path.
      if (!line.length) {
        process.stdout.write(JSON.stringify({__req_id: null, name: 'parse',
          success: false, error: 'blank line'}) + '\n');
        return;
      }
      process.stdout.write(runOne(line) + '\n');
    });
    rl.on('close', () => process.exit(0));
  } else {
    const input = fs.readFileSync(0, 'utf-8');
    process.stdout.write(runOne(input) + '\n');
  }
}

module.exports = {
  deriveSeed,
  _buildBattleForTest: buildBattle,
  _runScenarioForTest: handleExecuteTurns,
};

if (require.main === module) {
  main();
}
