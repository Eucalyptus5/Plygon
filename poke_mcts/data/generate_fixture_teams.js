'use strict';
const path = require('path');
const fs = require('fs');
const { convertMon } = require('./convert');
const Sim = require(path.join(__dirname, '..', '..', 'pokemon-showdown', 'dist', 'sim'));

const N = parseInt(process.argv[2] || '64', 10);
const teams = [];
for (let i = 0; i < N; i++) {
  const raw = Sim.Teams.generate('gen9randombattle', { seed: [0x5eed, 0, 0, i] });
  teams.push(raw.map(convertMon));
}
const out = path.join(__dirname, 'fixture_teams.json');
fs.writeFileSync(out, JSON.stringify({ meta: { n: N, seed_base: [0x5eed, 0, 0, 0] }, teams }, null, 0) + '\n');
console.log(`wrote ${teams.length} teams -> ${out}`);
