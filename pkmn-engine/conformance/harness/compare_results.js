#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const {
  enumerateActivePaths,
  enumerateSideConditionPaths,
  enumerateFieldPaths,
} = require('./lib/snapshot_paths');

const FORME_MAP_PATH = path.join(__dirname, '..', 'id_maps', 'engine_forme_to_showdown.json');

let _formeMapCache = null;
function loadFormeMap() {
  if (_formeMapCache !== null) return _formeMapCache;
  let map = {};
  if (fs.existsSync(FORME_MAP_PATH)) {
    const raw = fs.readFileSync(FORME_MAP_PATH, 'utf-8');
    // Parse error propagates; the CLI shim renders the historical message.
    map = JSON.parse(raw);
  }
  _formeMapCache = map;
  return map;
}

// The alive rule mirrors value-fidelity's sideValue: a positive species_id and a strictly false is_fainted.
function engineSideAlive(sideState) {
  if (!sideState || !Array.isArray(sideState.team)) return 0;
  let alive = 0;
  for (const m of sideState.team) {
    if (!m || !m.species_id) continue;
    if (m.is_fainted === false) alive++;
  }
  return alive;
}

// The engine has no winner field, so living-mon count is its outcome; a double-KO disagreement is deliberate.
function livingMonWinner(engResult) {
  const turns = engResult && engResult.turns;
  if (!Array.isArray(turns) || !turns.length) return null;
  const last = turns[turns.length - 1];
  const st = Array.isArray(last.state_after) ? last.state_after[0] : last.state_after;
  if (!st) return null;
  const a1 = engineSideAlive(st.p1), a2 = engineSideAlive(st.p2);
  if (a1 > 0 && a2 === 0) return 'p1';
  if (a2 > 0 && a1 === 0) return 'p2';
  if (a1 === 0 && a2 === 0) return 'tie';
  return null; // ongoing
}

// Applying the engine's living-mon rule to Showdown's snapshot separates a double-KO flip from a forfeit.
function winnerFidelityFields(sdResult, engResult) {
  if (!sdResult || !('winner' in sdResult)) return null;
  const sd = sdResult.winner; // 'p1'|'p2'|'tie'|null already mapped by the runner
  const sdByCount = livingMonWinner(sdResult); // same living-mon rule on SD's snapshot
  const eng = livingMonWinner(engResult);
  const decided = (v) => v === 'p1' || v === 'p2' || v === 'tie';
  const bothDecided = decided(sd) && decided(eng);
  return {
    sd_winner_side: sd,
    sd_winner_by_count: sdByCount,
    eng_winner_side: eng,
    winner_match: bothDecided ? (sd === eng) : null,
    both_decided: bothDecided,
  };
}

function firstDivergenceTurn(diffs, rejectInfo) {
  if (rejectInfo && typeof rejectInfo.turn_index === 'number') return rejectInfo.turn_index;
  let min = null;
  for (const d of (diffs || [])) {
    const m = /turns\[(\d+)\]/.exec(d.path || '');
    if (m) { const t = +m[1]; if (min === null || t < min) min = t; }
  }
  return min;
}

function compareResults(sdResult, engResult, formeMap) {
  const engine_forme_to_showdown = formeMap || {};
  const diffs = [];
  const warnings = [];

  function addDiff(path, showdown, engine) {
    diffs.push({ path, showdown, engine });
  }

  function addWarning(path, showdown, engine, note) {
    warnings.push({ path, showdown, engine, note });
  }

  function formeCollapseSuppress(engineSpeciesId, showdownSpeciesId) {
    const key = String(engineSpeciesId);
    return (key in engine_forme_to_showdown)
        && engine_forme_to_showdown[key].showdown_num === showdownSpeciesId;
  }

  function compareExecuteTurns() {
    const sdTurns = sdResult.turns || [];
    const engTurns = engResult.turns || [];

    if (sdTurns.length !== engTurns.length) {
      addDiff('turns.length', sdTurns.length, engTurns.length);
    }

    const minTurns = Math.min(sdTurns.length, engTurns.length);

    for (let i = 0; i < minTurns; i++) {
      const sd = sdTurns[i];
      const eng = engTurns[i];

      if (sd.state_after && eng.state_after) {
        compareSnapshots(`turns[${i}].state_after`, sd.state_after, eng.state_after);
      }

      if (sd.state_after_all_rolls && eng.state_after) {
        if (Array.isArray(eng.state_after)) {
          for (let r = 0; r < 16; r++) {
            if (sd.state_after_all_rolls[r] && eng.state_after[r]) {
              compareSnapshots(`turns[${i}].roll[${r}]`, sd.state_after_all_rolls[r], eng.state_after[r]);
            }
          }
        }
      }
    }
  }

  function compareSnapshots(prefix, sd, eng) {
    compareSideHP(`${prefix}.p1`, sd.p1, eng.p1);
    compareSideHP(`${prefix}.p2`, sd.p2, eng.p2);

    if (sd.field && eng.field) {
      for (const fp of enumerateFieldPaths(prefix, sd.field)) {
        if (fp.endsWith('.field.weather')) {
          if (normalizeWeather(sd.field.weather) !== eng.field.weather) {
            addDiff(fp, sd.field.weather, eng.field.weather);
          }
        } else if (fp.endsWith('.field.terrain')) {
          if (normalizeTerrain(sd.field.terrain) !== eng.field.terrain) {
            addDiff(fp, sd.field.terrain, eng.field.terrain);
          }
        }
      }
    }

    if (sd.p1?.active && eng.p1?.active) {
      compareActive(`${prefix}.p1.active`, sd.p1.active, eng.p1.active, sd.p1, eng.p1);
    }
    if (sd.p2?.active && eng.p2?.active) {
      compareActive(`${prefix}.p2.active`, sd.p2.active, eng.p2.active, sd.p2, eng.p2);
    }

    if (sd.p1?.side_conditions && eng.p1?.side_conditions) {
      compareSideConditions(`${prefix}.p1.side_conditions`, sd.p1.side_conditions, eng.p1.side_conditions);
    }
    if (sd.p2?.side_conditions && eng.p2?.side_conditions) {
      compareSideConditions(`${prefix}.p2.side_conditions`, sd.p2.side_conditions, eng.p2.side_conditions);
    }
  }

  function compareSideHP(prefix, sd, eng) {
    if (!sd || !eng) return;
    const sdTeam = sd.team || [];
    const engTeam = eng.team || [];

    const sdUsed = new Set();
    const unmatchedEngineSlots = [];
    let sawNullBuildSlot = false;

    for (let ei = 0; ei < engTeam.length; ei++) {
      const engMon = engTeam[ei];
      if ((engMon.species_id || 0) === 0) continue; // empty engine slot

      const sdIdx = sdTeam.findIndex((s, i) => !sdUsed.has(i) && s.build_slot === engMon.slot);
      if (sdIdx === -1) {
        const anyNull = sdTeam.some((s, i) => !sdUsed.has(i) && s.build_slot === null);
        if (anyNull) {
          sawNullBuildSlot = true;
          addWarning(
            `${prefix}.team[${ei}].build_slot`,
            null,
            engMon.slot,
            'pairing fell through because Showdown side had a null build_slot (name-prefix parse failed)',
          );
        } else {
          unmatchedEngineSlots.push(engMon.slot);
        }
        continue;
      }
      sdUsed.add(sdIdx);
      const sdMon = sdTeam[sdIdx];

      if (sdMon.species_id !== engMon.species_id) {
        const suppress = formeCollapseSuppress(engMon.species_id, sdMon.species_id);
        if (!suppress) {
          addWarning(
            `${prefix}.team[${ei}].species_id`,
            sdMon.species_id,
            engMon.species_id,
            'species_id mismatch on paired build_slot — not explained by engine_forme_to_showdown',
          );
        }
      }

      if (sdMon.current_hp !== engMon.current_hp) {
        addDiff(`${prefix}.team[${ei}].current_hp`, sdMon.current_hp, engMon.current_hp);
      }
      if (normalizeStatus(sdMon.status) !== normalizeStatus(engMon.status)) {
        addDiff(`${prefix}.team[${ei}].status`, sdMon.status, engMon.status);
      }
      if (sdMon.is_fainted !== engMon.is_fainted) {
        addDiff(`${prefix}.team[${ei}].is_fainted`, sdMon.is_fainted, engMon.is_fainted);
      }
      if ((sdMon.item_spritenum || 0) !== (engMon.item_id || 0)) {
        addDiff(`${prefix}.team[${ei}].item_id`, sdMon.item_spritenum || 0, engMon.item_id || 0);
      }
      if ((sdMon.ability_id || 0) !== (engMon.ability_id || 0)) {
        addDiff(`${prefix}.team[${ei}].ability_id`, sdMon.ability_id || 0, engMon.ability_id || 0);
      }
    }

    if (unmatchedEngineSlots.length > 0) {
      addDiff(`${prefix}.team.unpaired_count`, 0, unmatchedEngineSlots.length);
      addWarning(
        `${prefix}.team.unpaired_slots`,
        [],
        unmatchedEngineSlots,
        'engine slots with non-zero species_id and no matching Showdown build_slot — pairing FAIL',
      );
    } else if (sawNullBuildSlot) {
      // null build_slot: warning only, no FAIL row.
    }
  }

  function compareActive(prefix, sd, eng, sdSide, engSide) {
    if (!sd || !eng) return;

    const engEffective = engSide ? (engSide.effective || null) : null;
    const engActiveIndex = engSide ? (engSide.active_index ?? 0) : 0;
    const engActiveTeamMon = (engSide && engSide.team && engSide.team[engActiveIndex]) || null;

    for (const ap of enumerateActivePaths(prefix, sd)) {
      const tail = ap.slice(prefix.length + 1);

      if (tail.startsWith('boosts.')) {
        const k = tail.slice('boosts.'.length);
        if (!sd.boosts || !eng.boosts) continue;
        const sdVal = typeof sd.boosts === 'object' && !Array.isArray(sd.boosts)
          ? (sd.boosts[k] || 0)
          : 0;
        const boostKeys = ['atk', 'def', 'spa', 'spd', 'spe', 'accuracy', 'evasion'];
        const engIdx = boostKeys.indexOf(k);
        const engVal = Array.isArray(eng.boosts) ? (eng.boosts[engIdx] || 0) : (eng.boosts[k] || 0);
        if (sdVal !== engVal) addDiff(ap, sdVal, engVal);
        continue;
      }

      if (tail === 'substitute_hp') {
        const sdV = sd.substitute_hp || 0, engV = eng.substitute_hp || 0;
        if (sdV !== engV) addDiff(ap, sdV, engV);
      } else if (tail === 'confusion_turns') {
        const sdV = sd.confusion_turns || 0, engV = eng.confusion_turns || 0;
        if (sdV !== engV) addDiff(ap, sdV, engV);
      } else if (tail === 'taunt_turns') {
        const sdV = sd.taunt_turns || 0, engV = eng.taunt_turns || 0;
        if (sdV !== engV) addDiff(ap, sdV, engV);
      } else if (tail === 'encore_turns') {
        const sdV = sd.encore_turns || 0, engV = eng.encore_turns || 0;
        if (sdV !== engV) addDiff(ap, sdV, engV);
      } else if (tail === 'is_terastallized') {
        const sdTera = !!sd.is_terastallized;
        const engTera = engActiveTeamMon ? !!engActiveTeamMon.is_terastallized : false;
        if (sdTera !== engTera) addDiff(ap, sdTera, engTera);
      } else if (tail === 'types') {
        if (engEffective && Array.isArray(engEffective.types) && Array.isArray(sd.types)) {
          const sdTypes = [...new Set(sd.types)].sort();
          const engTypes = [...new Set(engEffective.types)].sort();
          if (sdTypes.length !== engTypes.length || sdTypes.some((t, i) => t !== engTypes[i])) {
            addDiff(ap, sdTypes, engTypes);
          }
        }
      } else if (tail === 'effective_ability_id') {
        if (engEffective) {
          const sdV = sd.effective_ability_id || 0, engV = engEffective.ability || 0;
          if (sdV !== engV) addDiff(ap, sdV, engV);
        }
      } else if (tail === 'effective_item_id') {
        if (engActiveTeamMon) {
          const sdV = sd.effective_item_id || 0, engV = engActiveTeamMon.item_id || 0;
          if (sdV !== engV) addDiff(ap, sdV, engV);
        }
      } else if (tail === 'effective_species_id') {
        if (engEffective) {
          const sdV = sd.effective_species_id || 0, engV = engEffective.species || 0;
          if (sdV !== engV && !formeCollapseSuppress(engV, sdV)) addDiff(ap, sdV, engV);
        }
      }
    }
  }

  function compareSideConditions(prefix, sd, eng) {
    const sdNorm = {};
    for (const [key, val] of Object.entries(sd)) {
      sdNorm[key] = typeof val === 'object' ? (val.layers || 1) : val;
    }

    const SD_HAZARD_KEYS = {
      stealth_rock: 'stealthrock',
      spikes: 'spikes',
      toxic_spikes: 'toxicspikes',
      sticky_web: 'stickyweb',
    };
    const SD_SCREEN_KEYS = {
      reflect_turns: 'reflect',
      light_screen_turns: 'lightscreen',
      aurora_veil_turns: 'auroraveil',
    };

    for (const sp of enumerateSideConditionPaths(prefix, sd)) {
      const tail = sp.slice(prefix.length + 1);

      if (tail in SD_HAZARD_KEYS) {
        const sdKey = SD_HAZARD_KEYS[tail];
        const sdVal = sdNorm[sdKey] ? (typeof sdNorm[sdKey] === 'boolean' ? 1 : sdNorm[sdKey]) : 0;
        const engVal = eng[tail] || 0;
        const sdNum = sdVal === true ? 1 : (sdVal === false ? 0 : sdVal);
        const engNum = engVal === true ? 1 : (engVal === false ? 0 : engVal);
        if (sdNum !== engNum) addDiff(sp, sdNum, engNum);
      } else if (tail in SD_SCREEN_KEYS) {
        const sdKey = SD_SCREEN_KEYS[tail];
        const sdVal = sd[sdKey] ? (sd[sdKey].duration || 0) : 0;
        const engVal = eng[tail] || 0;
        if ((sdVal > 0) !== (engVal > 0)) addDiff(sp, sdVal, engVal);
      }
    }
  }

  function compareCalcDamage() {
    const sdRolls = sdResult.results?.all_rolls || [];
    const engRolls = engResult.results?.all_rolls || [];

    const sdDamages = sdRolls.map(r => r.damage).sort((a, b) => a - b);
    const engDamages = engRolls.map(r => r.damage).sort((a, b) => a - b);

    if (sdDamages.length !== engDamages.length) {
      addDiff('results.all_rolls.length', sdDamages.length, engDamages.length);
    }

    for (let i = 0; i < Math.min(sdDamages.length, engDamages.length); i++) {
      if (sdDamages[i] !== engDamages[i]) {
        addDiff(`results.all_rolls[${i}].damage`, sdDamages[i], engDamages[i]);
      }
    }

    if (sdResult.results?.min_damage !== engResult.results?.min_damage) {
      addDiff('results.min_damage', sdResult.results?.min_damage, engResult.results?.min_damage);
    }
    if (sdResult.results?.max_damage !== engResult.results?.max_damage) {
      addDiff('results.max_damage', sdResult.results?.max_damage, engResult.results?.max_damage);
    }

    const sdCrits = (sdResult.results?.crit_results || []).map(r => r.damage).sort((a, b) => a - b);
    const engCrits = (engResult.results?.crit_results || []).map(r => r.damage).sort((a, b) => a - b);

    for (let i = 0; i < Math.min(sdCrits.length, engCrits.length); i++) {
      if (sdCrits[i] !== engCrits[i]) {
        addDiff(`results.crit_results[${i}].damage`, sdCrits[i], engCrits[i]);
      }
    }
  }

  const mode = sdResult.mode || engResult.mode || 'execute_turns';

  // A non-forced byte 255 is a replay-contract violation, not an engine divergence, so it books like a reject.
  const CONTRACT_STRUGGLE255 = 'contract:struggle255_not_forced';
  const isStruggle255 = (sdResult.success === false && sdResult.error === CONTRACT_STRUGGLE255)
    || (sdResult.reject_info != null && sdResult.reject_info.reason === CONTRACT_STRUGGLE255);
  // After a reject Showdown recovers on a board the engine never saw, so state comparison is skipped.
  const sdRejected = mode !== 'calc_damage' && (
    (sdResult.success === false && sdResult.error === 'Not all choices done')
    || isStruggle255
    || (sdResult.reject_info != null)
  );
  if (sdRejected) {
    const wf = winnerFidelityFields(sdResult, engResult);
    return {
      scenario: sdResult.name || engResult.name || 'unknown',
      mode,
      verdict: 'PASS',
      diff_count: 0,
      diffs: [],
      note: isStruggle255
        ? 'non-forced byte-255 contract reject; state comparison skipped'
        : 'showdown rejected scenario as illegal; state comparison skipped',
      reject: true,
      reject_reason: isStruggle255 ? CONTRACT_STRUGGLE255 : undefined,
      first_divergence: firstDivergenceTurn(null, sdResult.reject_info),
      ...(wf || {}),
    };
  }

  if (mode === 'calc_damage') {
    compareCalcDamage();
  } else {
    compareExecuteTurns();
  }

  const verdict = diffs.length === 0 ? 'PASS' : 'FAIL';
  const wf = winnerFidelityFields(sdResult, engResult);
  return {
    scenario: sdResult.name || engResult.name || 'unknown',
    mode,
    verdict,
    diff_count: diffs.length,
    diffs,
    warning_count: warnings.length,
    warnings,
    ...(wf ? { ...wf, first_divergence: firstDivergenceTurn(diffs, null) } : {}),
  };
}

function normalizeStatus(s) {
  if (!s || s === 'none' || s === '') return 'none';
  const map = { 'brn': 'burn', 'par': 'paralysis', 'psn': 'poison', 'tox': 'bad_poison', 'slp': 'sleep', 'frz': 'freeze', 'fnt': 'none' };
  return map[s] || s;
}

function normalizeWeather(w) {
  if (!w || w === '' || w === 'none') return 'none';
  const map = {
    'sunnyday': 'sun', 'SunnyDay': 'sun', 'sun': 'sun',
    'raindance': 'rain', 'RainDance': 'rain', 'rain': 'rain',
    'sandstorm': 'sand', 'Sandstorm': 'sand', 'sand': 'sand', 'sandstream': 'sand',
    'snow': 'snow', 'snowscape': 'snow',
    'desolateland': 'harsh_sun', 'harsh_sun': 'harsh_sun',
    'primordialsea': 'heavy_rain', 'heavy_rain': 'heavy_rain',
    'deltastream': 'strong_winds', 'strong_winds': 'strong_winds',
  };
  return map[w.toLowerCase()] || w;
}

function normalizeTerrain(t) {
  if (!t || t === '' || t === 'none') return 'none';
  const map = {
    'electricterrain': 'electric', 'Electric Terrain': 'electric', 'electric': 'electric',
    'grassyterrain': 'grassy', 'Grassy Terrain': 'grassy', 'grassy': 'grassy',
    'psychicterrain': 'psychic', 'Psychic Terrain': 'psychic', 'psychic': 'psychic',
    'mistyterrain': 'misty', 'Misty Terrain': 'misty', 'misty': 'misty',
  };
  return map[t.toLowerCase()] || t;
}

module.exports = { compareResults, loadFormeMap, normalizeStatus };

if (require.main === module) {
  if (process.argv.length < 4) {
    console.error('Usage: node compare_results.js <showdown.json> <engine.json>');
    process.exit(1);
  }

  const sdResult = JSON.parse(fs.readFileSync(process.argv[2], 'utf-8'));
  const engResult = JSON.parse(fs.readFileSync(process.argv[3], 'utf-8'));

  let formeMap;
  try {
    formeMap = loadFormeMap();
  } catch (e) {
    console.error(`compare_results.js: failed to parse ${FORME_MAP_PATH}: ${e.message}`);
    process.exit(1);
  }

  const output = compareResults(sdResult, engResult, formeMap);
  console.log(JSON.stringify(output, null, 2));
  process.exit(output.verdict === 'PASS' ? 0 : 1);
}
