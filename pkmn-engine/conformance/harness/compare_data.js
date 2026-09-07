#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

const ID_MAPS = path.join(__dirname, '..', 'id_maps');
const RESULTS = path.join(__dirname, '..', 'results', 'data_compare');
fs.mkdirSync(RESULTS, { recursive: true });

const engineData = JSON.parse(fs.readFileSync(0, 'utf-8'));

const showdownMoves = JSON.parse(fs.readFileSync(path.join(ID_MAPS, 'showdown_moves.json'), 'utf-8'));
const showdownSpecies = JSON.parse(fs.readFileSync(path.join(ID_MAPS, 'showdown_species.json'), 'utf-8'));
const showdownItems = JSON.parse(fs.readFileSync(path.join(ID_MAPS, 'showdown_items.json'), 'utf-8'));

const moveMap = JSON.parse(fs.readFileSync(path.join(ID_MAPS, 'move_map.json'), 'utf-8'));
const speciesMap = JSON.parse(fs.readFileSync(path.join(ID_MAPS, 'species_map.json'), 'utf-8'));
const itemMap = JSON.parse(fs.readFileSync(path.join(ID_MAPS, 'item_map.json'), 'utf-8'));

const args = process.argv.slice(2);
const doAll = args.length === 0;
const doMoves = doAll || args.includes('--moves');
const doSpecies = doAll || args.includes('--species');
const doItems = doAll || args.includes('--items');

const SD_TYPE_TO_ENGINE = {
  'Normal': 'Normal', 'Fighting': 'Fighting', 'Flying': 'Flying',
  'Poison': 'Poison', 'Ground': 'Ground', 'Rock': 'Rock',
  'Bug': 'Bug', 'Ghost': 'Ghost', 'Steel': 'Steel',
  'Fire': 'Fire', 'Water': 'Water', 'Grass': 'Grass',
  'Electric': 'Electric', 'Psychic': 'Psychic', 'Ice': 'Ice',
  'Dragon': 'Dragon', 'Dark': 'Dark', 'Fairy': 'Fairy',
};

const SD_CATEGORY_TO_ENGINE = {
  'Physical': 'Physical', 'Special': 'Special', 'Status': 'Status',
};

function compareMoves() {
  const mismatches = [];
  const missing = [];
  let compared = 0;

  for (const [sdKey, sdNum] of Object.entries(moveMap)) {
    const sd = showdownMoves[sdKey];
    if (!sd) continue;

    const eng = engineData.moves[sdNum];
    if (!eng) {
      missing.push({ showdown_key: sdKey, showdown_num: sdNum, reason: 'not_in_engine' });
      continue;
    }

    compared++;
    const diffs = [];

    if (sd.basePower !== eng.base_power) {
      diffs.push({ field: 'base_power', showdown: sd.basePower, engine: eng.base_power });
    }

    // Showdown spells never-miss accuracy as true, the engine as 0.
    const sdAcc = sd.accuracy === true ? 0 : sd.accuracy;
    if (sdAcc !== eng.accuracy) {
      diffs.push({ field: 'accuracy', showdown: sd.accuracy, engine: eng.accuracy });
    }

    const expectedType = SD_TYPE_TO_ENGINE[sd.type];
    if (expectedType && expectedType !== eng.move_type) {
      diffs.push({ field: 'type', showdown: sd.type, engine: eng.move_type });
    }

    const expectedCat = SD_CATEGORY_TO_ENGINE[sd.category];
    if (expectedCat && expectedCat !== eng.category) {
      diffs.push({ field: 'category', showdown: sd.category, engine: eng.category });
    }

    if (sd.priority !== eng.priority) {
      diffs.push({ field: 'priority', showdown: sd.priority, engine: eng.priority });
    }

    if (sd.pp !== eng.pp) {
      diffs.push({ field: 'pp', showdown: sd.pp, engine: eng.pp });
    }

    // Showdown's critRatio is 1-based where the engine's crit_ratio is 0-based, so they differ by one.
    const sdCrit = sd.critRatio || 1;
    if (sdCrit !== eng.crit_ratio + 1) {
      diffs.push({ field: 'crit_ratio', showdown: sdCrit, engine: eng.crit_ratio, note: 'sd=1-based, eng=0-based' });
    }

    const sdSecondaryChance = sd.secondary?.chance || (sd.secondaries ? sd.secondaries[0]?.chance : 0) || 0;
    if (sdSecondaryChance !== eng.secondary_chance) {
      diffs.push({ field: 'secondary_chance', showdown: sdSecondaryChance, engine: eng.secondary_chance });
    }

    const sdFlags = sd.flags || {};
    const engineFlagNames = eng.flag_names || [];
    const flagMapping = {
      'contact': 'contact', 'sound': 'sound', 'punch': 'punch',
      'pulse': 'pulse', 'bite': 'bite', 'powder': 'powder',
      'dance': 'dance', 'wind': 'wind', 'slicing': 'slice',
      'protect': 'protect', 'reflectable': 'reflectable',
      'recharge': 'recharge', 'charge': 'charge', 'heal': 'heal',
      'bypasssub': 'bypasssub', 'bullet': 'bullet',
    };
    for (const [sdFlag, engFlag] of Object.entries(flagMapping)) {
      const sdHas = !!sdFlags[sdFlag];
      const engHas = engineFlagNames.includes(engFlag);
      if (sdHas !== engHas) {
        diffs.push({ field: `flag_${engFlag}`, showdown: sdHas, engine: engHas });
      }
    }

    const sdMulti = sd.multihit;
    if (sdMulti) {
      const [lo, hi] = Array.isArray(sdMulti) ? sdMulti : [sdMulti, sdMulti];
      if (lo !== eng.multihit_lo || hi !== eng.multihit_hi) {
        diffs.push({ field: 'multihit', showdown: [lo, hi], engine: [eng.multihit_lo, eng.multihit_hi] });
      }
    } else if (eng.multihit_lo !== 0 || eng.multihit_hi !== 0) {
      diffs.push({ field: 'multihit', showdown: null, engine: [eng.multihit_lo, eng.multihit_hi] });
    }

    if (diffs.length > 0) {
      mismatches.push({
        move: sdKey,
        num: sdNum,
        name: sd.name,
        diffs,
      });
    }
  }

  const missingEffects = [];
  for (const [sdKey, sdNum] of Object.entries(moveMap)) {
    const sd = showdownMoves[sdKey];
    if (!sd) continue;
    const eng = engineData.moves[sdNum];
    if (!eng) continue;

    const hasHandler = sd.hasOnHit || sd.hasOnAfterHit || sd.hasOnModifyMove ||
                       sd.hasOnMoveFail || sd.hasOnTryHit;
    if (hasHandler && eng.effect === 'None' && eng.self_effect === 'None') {
      missingEffects.push({
        move: sdKey,
        num: sdNum,
        name: sd.name,
        handlers: {
          onHit: sd.hasOnHit,
          onAfterHit: sd.hasOnAfterHit,
          onModifyMove: sd.hasOnModifyMove,
          onMoveFail: sd.hasOnMoveFail,
          onTryHit: sd.hasOnTryHit,
        },
        engine_effect: eng.effect,
      });
    }
  }

  const result = { compared, mismatches: mismatches.length, missing: missing.length, missingEffects: missingEffects.length };
  fs.writeFileSync(path.join(RESULTS, 'move_mismatches.json'), JSON.stringify({ summary: result, mismatches, missing, missingEffects }, null, 2));
  console.log(`Moves: ${compared} compared, ${mismatches.length} mismatches, ${missing.length} missing, ${missingEffects.length} missing effects`);
}

function compareSpecies() {
  const mismatches = [];
  const missing = [];
  let compared = 0;

  for (const [sdKey, sdNum] of Object.entries(speciesMap)) {
    const sd = showdownSpecies[sdKey];
    if (!sd) continue;

    // Alternate formes share their base num, so only base formes are compared.
    if (sd.forme && sd.forme !== '') continue;

    const eng = engineData.species[sdNum];
    if (!eng) {
      missing.push({ showdown_key: sdKey, showdown_num: sdNum, reason: 'not_in_engine' });
      continue;
    }

    compared++;
    const diffs = [];

    const statNames = ['hp', 'atk', 'def', 'spa', 'spd', 'spe'];
    for (const stat of statNames) {
      const sdVal = sd.baseStats[stat];
      const engVal = eng[stat];
      // Engine uses u8, so stats > 255 would be truncated
      const expected = sdVal > 255 ? sdVal & 0xFF : sdVal;
      if (expected !== engVal) {
        diffs.push({ field: `base_${stat}`, showdown: sdVal, engine: engVal, truncated: sdVal > 255 });
      }
    }

    const sdType1 = sd.types[0] || 'Normal';
    const sdType2 = sd.types[1] || sd.types[0] || 'Normal';
    if (SD_TYPE_TO_ENGINE[sdType1] !== eng.type1) {
      diffs.push({ field: 'type1', showdown: sdType1, engine: eng.type1 });
    }
    if (SD_TYPE_TO_ENGINE[sdType2] !== eng.type2) {
      diffs.push({ field: 'type2', showdown: sdType2, engine: eng.type2 });
    }

    // The engine's weight unit is unconfirmed, so either hectograms or whole kilograms counts as a match.
    const sdWeightHg = Math.round(sd.weightkg * 10); // hectograms
    if (sdWeightHg !== eng.weight && Math.round(sd.weightkg) !== eng.weight) {
      diffs.push({ field: 'weight', showdown_kg: sd.weightkg, showdown_hg: sdWeightHg, engine: eng.weight });
    }

    if (diffs.length > 0) {
      mismatches.push({
        species: sdKey,
        num: sdNum,
        name: sd.name,
        diffs,
      });
    }
  }

  const result = { compared, mismatches: mismatches.length, missing: missing.length };
  fs.writeFileSync(path.join(RESULTS, 'species_mismatches.json'), JSON.stringify({ summary: result, mismatches, missing }, null, 2));
  console.log(`Species: ${compared} compared, ${mismatches.length} mismatches, ${missing.length} missing`);
}

function compareItems() {
  const mismatches = [];
  const missing = [];
  let compared = 0;

  for (const [sdKey, spritenum] of Object.entries(itemMap)) {
    const sd = showdownItems[sdKey];
    if (!sd) continue;

    const eng = engineData.items[spritenum];
    if (!eng) {
      missing.push({ showdown_key: sdKey, spritenum, reason: 'not_in_engine_array' });
      continue;
    }

    if (eng.is_none) {
      // An item the engine models as NONE only counts as missing when Showdown gives it gameplay handlers.
      const hasHandlers = sd.hasOnModifyAtk || sd.hasOnModifyDef || sd.hasOnModifySpa ||
                          sd.hasOnModifySpd || sd.hasOnModifySpe || sd.hasOnModifyDamage ||
                          sd.hasOnBasePower || sd.hasOnResidual || sd.hasOnAfterMoveSecondary ||
                          sd.hasOnDamagingHit || sd.hasOnEat;
      if (hasHandlers) {
        missing.push({ showdown_key: sdKey, spritenum, name: sd.name, reason: 'engine_has_no_flags', handlers: {
          onModifyAtk: sd.hasOnModifyAtk, onModifyDef: sd.hasOnModifyDef,
          onModifySpa: sd.hasOnModifySpa, onModifySpd: sd.hasOnModifySpd,
          onModifySpe: sd.hasOnModifySpe, onModifyDamage: sd.hasOnModifyDamage,
          onBasePower: sd.hasOnBasePower, onResidual: sd.hasOnResidual,
          onDamagingHit: sd.hasOnDamagingHit, onEat: sd.hasOnEat,
        }});
      }
      continue;
    }

    compared++;
    // Items are only counted here; no per-field comparison is done.
  }

  const result = { compared, mismatches: mismatches.length, missing: missing.length };
  fs.writeFileSync(path.join(RESULTS, 'item_mismatches.json'), JSON.stringify({ summary: result, mismatches, missing }, null, 2));
  console.log(`Items: ${compared} compared, ${mismatches.length} mismatches, ${missing.length} missing`);
}

if (doMoves) compareMoves();
if (doSpecies) compareSpecies();
if (doItems) compareItems();
