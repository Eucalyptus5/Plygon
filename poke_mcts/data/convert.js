'use strict';
const path = require('path');
const fs = require('fs');

const MAPS = path.join(__dirname, '..', '..', 'testing_plan', 'id_maps');
const speciesMap = JSON.parse(fs.readFileSync(path.join(MAPS, 'species_map.json')));
const moveMap = JSON.parse(fs.readFileSync(path.join(MAPS, 'move_map.json')));
const itemMap = JSON.parse(fs.readFileSync(path.join(MAPS, 'item_map.json')));
const abilityMap = JSON.parse(fs.readFileSync(path.join(MAPS, 'ability_map.json')));

// Engine scenario convention: tera_type is a SHOWDOWN type index in this order
// (testing_plan/harness/showdown_runner.js TYPE_NAMES).
const TYPE_NAMES = [
  'Normal', 'Fighting', 'Flying', 'Poison', 'Ground', 'Rock',
  'Bug', 'Ghost', 'Steel', 'Fire', 'Water', 'Grass',
  'Electric', 'Psychic', 'Ice', 'Dragon', 'Dark', 'Fairy',
  'Stellar', // index 18: gen9 randbats emits Terapagos with teraType "Stellar"
];

const toID = (s) => ('' + s).toLowerCase().replace(/[^a-z0-9]/g, '');

function convertMon(m) {
  const sid = speciesMap[m.speciesId ?? toID(m.species)];
  if (sid === undefined) throw new Error(`unmapped species: ${m.species}`);
  const aid = abilityMap[toID(m.ability)];
  if (aid === undefined) throw new Error(`unmapped ability: ${m.ability} (${m.species})`);
  const itemId = m.item ? itemMap[toID(m.item)] : 0;
  if (m.item && itemId === undefined) throw new Error(`unmapped item: ${m.item}`);
  const moves = m.moves.map((mv) => {
    const id = moveMap[toID(mv)];
    if (id === undefined) throw new Error(`unmapped move: ${mv} (${m.species})`);
    return id;
  });
  while (moves.length < 4) moves.push(0);
  const tera = TYPE_NAMES.indexOf(m.teraType);
  if (tera < 0) throw new Error(`unmapped tera type: ${m.teraType}`);
  const order = ['hp', 'atk', 'def', 'spa', 'spd', 'spe'];
  return {
    species_id: sid,
    ability_id: aid,
    item_id: itemId ?? 0,
    moves: moves.slice(0, 4),
    ivs: order.map((k) => m.ivs?.[k] ?? 31),
    evs: order.map((k) => m.evs?.[k] ?? 85),
    nature: 0, // gen9 randbats sets carry no nature -> neutral
    level: m.level ?? 100,
    tera_type: tera,
    is_female: m.gender === 'F',
  };
}

module.exports = { convertMon, toID };
