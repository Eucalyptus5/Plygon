#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const {Dex} = require(require('./lib/showdown_dir.js').showdownSim());

const OUT_DIR = path.join(__dirname, '..', 'id_maps');
fs.mkdirSync(OUT_DIR, { recursive: true });

const dex = Dex.mod('gen9');

function isGen9Current(entry) {
  const ns = entry.isNonstandard;
  if (ns === 'Past' || ns === 'CAP' || ns === 'LGPE' || ns === 'Future' || ns === 'Unobtainable') return false;
  return true;
}

function toID(s) {
  return String(s).toLowerCase().replace(/[^a-z0-9]/g, '');
}

const GEN_SPECIES_RS = path.join(__dirname, '..', '..', 'src', 'data', 'generated', 'gen_species.rs');
const speciesRsText = fs.readFileSync(GEN_SPECIES_RS, 'utf-8');
const speciesRsLines = speciesRsText.split('\n');

const FORME_LINE_RE = /^\s*arr\[(\d+)\]\s*=\s*(\d+);\s*\/\/\s*(.+)$/;
const ARR_LINE_RE = /^\s*arr\[/;

const showdownNumByEngineForme = {};
const formeTableByToId = {};
const formeTableEntries = [];

let arrLineCount = 0;
let parsedLineCount = 0;
for (const line of speciesRsLines) {
  if (!ARR_LINE_RE.test(line)) continue;
  const m = FORME_LINE_RE.exec(line);
  if (!m) continue;
  const engineId = parseInt(m[1], 10);
  const showdownNum = parseInt(m[2], 10);
  const formeName = m[3].trim();
  if (engineId < 1100 || engineId > 1453) continue;
  arrLineCount++;
  parsedLineCount++;
  if (/-Mega|-Gmax/i.test(formeName)) continue;
  showdownNumByEngineForme[engineId] = showdownNum;
  formeTableByToId[toID(formeName)] = { engineId, showdownNum, formeName };
  formeTableEntries.push({ engineId, showdownNum, formeName });
}

let arrLineCountStrict = 0;
for (const line of speciesRsLines) {
  if (!ARR_LINE_RE.test(line)) continue;
  const head = /^\s*arr\[(\d+)\]/.exec(line);
  if (!head) continue;
  const id = parseInt(head[1], 10);
  if (id < 1100 || id > 1453) continue;
  arrLineCountStrict++;
}

let parsedLineCountStrict = 0;
for (const line of speciesRsLines) {
  if (!ARR_LINE_RE.test(line)) continue;
  if (!FORME_LINE_RE.test(line)) continue;
  const head = /^\s*arr\[(\d+)\]/.exec(line);
  if (!head) continue;
  const id = parseInt(head[1], 10);
  if (id < 1100 || id > 1453) continue;
  parsedLineCountStrict++;
}

if (arrLineCountStrict !== parsedLineCountStrict) {
  console.error(`generate_maps.js: forme-table parse cardinality mismatch. arr[ rows=${arrLineCountStrict}, regex-matched rows=${parsedLineCountStrict}.`);
  process.exit(2);
}

const moveMap = {};
const showdownMoves = {};

for (const move of dex.moves.all()) {
  if (move.num <= 0) continue;
  if (!isGen9Current(move)) continue;

  moveMap[move.id] = move.num;

  showdownMoves[move.id] = {
    num: move.num,
    name: move.name,
    basePower: move.basePower,
    accuracy: move.accuracy,
    type: move.type,
    category: move.category,
    priority: move.priority,
    pp: move.pp,
    flags: move.flags,
    secondary: move.secondary || null,
    secondaries: move.secondaries || null,
    drain: move.drain || null,
    recoil: move.recoil || null,
    multihit: move.multihit || null,
    critRatio: move.critRatio || 1,
    target: move.target,
    volatileStatus: move.volatileStatus || null,
    sideCondition: move.sideCondition || null,
    weather: move.weather || null,
    terrain: move.terrain || null,
    pseudoWeather: move.pseudoWeather || null,
    status: move.status || null,
    boosts: move.boosts || null,
    selfBoost: move.selfBoost || null,
    self: move.self || null,
    hasCrashDamage: move.hasCrashDamage || false,
    willCrit: move.willCrit || false,
    forceSwitch: move.forceSwitch || false,
    selfSwitch: move.selfSwitch || false,
    breaksProtect: move.breaksProtect || false,
    isZ: move.isZ || false,
    isMax: move.isMax || false,
    noPPBoosts: move.noPPBoosts || false,
    hasOnHit: typeof move.onHit === 'function',
    hasOnAfterHit: typeof move.onAfterHit === 'function',
    hasOnModifyMove: typeof move.onModifyMove === 'function',
    hasOnMoveFail: typeof move.onMoveFail === 'function',
    hasOnTryHit: typeof move.onTryHit === 'function',
    hasOnAfterSubDamage: typeof move.onAfterSubDamage === 'function',
    hasOnAfterMoveSecondarySelf: typeof move.onAfterMoveSecondarySelf === 'function',
    hasOnBasePower: typeof move.onBasePower === 'function',
    hasOnEffectiveness: typeof move.onEffectiveness === 'function',
  };
}

const speciesMap = {};
const showdownSpecies = {};
const engineFormeToShowdown = {};

for (const species of dex.species.all()) {
  if (species.num <= 0) continue;

  const formeEntry = formeTableByToId[species.id];
  if (!formeEntry) {
    if (!isGen9Current(species)) continue;
    speciesMap[species.id] = species.num;
  } else {
    speciesMap[species.id] = formeEntry.engineId;
    engineFormeToShowdown[String(formeEntry.engineId)] = {
      showdown_num: formeEntry.showdownNum,
      showdown_forme_name: formeEntry.formeName,
    };
  }

  showdownSpecies[species.id] = {
    num: species.num,
    name: species.name,
    baseStats: species.baseStats,
    types: species.types,
    weightkg: species.weightkg,
    abilities: species.abilities,
    forme: species.forme || '',
    baseSpecies: species.baseSpecies || species.name,
    evos: species.evos || [],
    prevo: species.prevo || null,
    cosmeticFormes: species.cosmeticFormes || null,
    formeOrder: species.formeOrder || null,
    otherFormes: species.otherFormes || null,
  };
}

const unresolvedFormeEntries = [];
for (const entry of formeTableEntries) {
  if (!(String(entry.engineId) in engineFormeToShowdown)) {
    unresolvedFormeEntries.push(entry);
  }
}
if (unresolvedFormeEntries.length > 0) {
  console.error('generate_maps.js: unresolved forme-table entries (no Showdown species.id matched toID(formeName)):');
  for (const e of unresolvedFormeEntries) {
    console.error(`  arr[${e.engineId}] = ${e.showdownNum}; // ${e.formeName} -> toID="${toID(e.formeName)}"`);
  }
  process.exit(3);
}

const silvallyCount = formeTableEntries.filter(e => /^Silvally-/.test(e.formeName)).length;
if (silvallyCount !== 17) {
  console.error(`generate_maps.js: expected 17 Silvally formes in engine table, found ${silvallyCount}.`);
  process.exit(4);
}

const regionalProbeIds = ['rattataalola', 'mrmimegalar', 'farfetchdgalar', 'ponytagalar'];
for (const id of regionalProbeIds) {
  if (speciesMap[id] === undefined) {
    console.error(`generate_maps.js: species_map['${id}'] missing — Past-tagged regional did not resolve.`);
    process.exit(5);
  }
}

const itemMap = {};
const showdownItems = {};

for (const item of dex.items.all()) {
  if (item.num <= 0) continue;
  if (!isGen9Current(item)) continue;

  // The engine uses spritenum as its item index.
  itemMap[item.id] = item.spritenum;

  showdownItems[item.id] = {
    num: item.num,
    spritenum: item.spritenum,
    name: item.name,
    fling: item.fling || null,
    onPlate: item.onPlate || null,
    onDrive: item.onDrive || null,
    onMemory: item.onMemory || null,
    forcedForme: item.forcedForme || null,
    megaStone: item.megaStone || null,
    zMove: item.zMove || null,
    zMoveType: item.zMoveType || null,
    naturalGift: item.naturalGift || null,
    isPokeball: item.isPokeball || false,
    boosts: item.boosts || null,
    hasOnModifyAtk: typeof item.onModifyAtk === 'function',
    hasOnModifyDef: typeof item.onModifyDef === 'function',
    hasOnModifySpa: typeof item.onModifySpa === 'function',
    hasOnModifySpd: typeof item.onModifySpd === 'function',
    hasOnModifySpe: typeof item.onModifySpe === 'function',
    hasOnModifyDamage: typeof item.onModifyDamage === 'function',
    hasOnBasePower: typeof item.onBasePower === 'function',
    hasOnResidual: typeof item.onResidual === 'function',
    hasOnAfterMoveSecondary: typeof item.onAfterMoveSecondary === 'function',
    hasOnDamagingHit: typeof item.onDamagingHit === 'function',
    hasOnEat: typeof item.onEat === 'function',
  };
}

const abilityMap = {};
const showdownAbilities = {};

for (const ability of dex.abilities.all()) {
  if (ability.num <= 0) continue;
  if (!isGen9Current(ability)) continue;

  abilityMap[ability.id] = ability.num;

  showdownAbilities[ability.id] = {
    num: ability.num,
    name: ability.name,
    hasOnModifyAtk: typeof ability.onModifyAtk === 'function',
    hasOnModifyDef: typeof ability.onModifyDef === 'function',
    hasOnModifySpa: typeof ability.onModifySpa === 'function',
    hasOnModifySpd: typeof ability.onModifySpd === 'function',
    hasOnModifySpe: typeof ability.onModifySpe === 'function',
    hasOnModifyDamage: typeof ability.onModifyDamage === 'function',
    hasOnBasePower: typeof ability.onBasePower === 'function',
    hasOnDamagingHit: typeof ability.onDamagingHit === 'function',
    hasOnResidual: typeof ability.onResidual === 'function',
    hasOnSwitchIn: typeof ability.onSwitchIn === 'function',
    hasOnStart: typeof ability.onStart === 'function',
    hasOnTryHit: typeof ability.onTryHit === 'function',
    hasOnSourceModifyDamage: typeof ability.onSourceModifyDamage === 'function',
    hasOnFoeTrapPokemon: typeof ability.onFoeTrapPokemon === 'function',
    hasOnModifyMove: typeof ability.onModifyMove === 'function',
    hasOnBeforeMove: typeof ability.onBeforeMove === 'function',
  };
}

function writeJSON(filename, data) {
  fs.writeFileSync(path.join(OUT_DIR, filename), JSON.stringify(data, null, 2) + '\n');
}

writeJSON('move_map.json', moveMap);
writeJSON('species_map.json', speciesMap);
writeJSON('item_map.json', itemMap);
writeJSON('ability_map.json', abilityMap);
writeJSON('engine_forme_to_showdown.json', engineFormeToShowdown);
writeJSON('showdown_moves.json', showdownMoves);
writeJSON('showdown_species.json', showdownSpecies);
writeJSON('showdown_items.json', showdownItems);
writeJSON('showdown_abilities.json', showdownAbilities);

console.log(`Moves:           ${Object.keys(moveMap).length}`);
console.log(`Species:         ${Object.keys(speciesMap).length}`);
console.log(`Items:           ${Object.keys(itemMap).length}`);
console.log(`Abilities:       ${Object.keys(abilityMap).length}`);
console.log(`Forme reverse:   ${Object.keys(engineFormeToShowdown).length}`);
console.log(`Forme table arr[ rows: ${arrLineCountStrict}; regex-matched rows: ${parsedLineCountStrict} (post-Mega/Gmax filter retains ${formeTableEntries.length}).`);
console.log(`Output: ${OUT_DIR}`);
