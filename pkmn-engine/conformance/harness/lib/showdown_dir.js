'use strict';

const fs = require('fs');
const path = require('path');

const CONFORMANCE_DIR = path.resolve(__dirname, '..', '..');
const CONFIG_PATH = path.join(CONFORMANCE_DIR, 'fuzzer', 'config.json');

function isBuilt(dir) {
  return fs.existsSync(path.join(dir, 'dist', 'sim'));
}

function explicit() {
  if (process.env.SHOWDOWN_DIR) {
    return { source: 'SHOWDOWN_DIR', dir: path.resolve(process.env.SHOWDOWN_DIR) };
  }
  let cfg = null;
  try { cfg = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8')); } catch (_) { return null; }
  if (typeof cfg.showdown_dir === 'string' && cfg.showdown_dir.length > 0) {
    return { source: 'config.json showdown_dir', dir: path.resolve(path.dirname(CONFIG_PATH), cfg.showdown_dir) };
  }
  return null;
}

const DEFAULTS = [
  path.resolve(CONFORMANCE_DIR, '..', 'pokemon-showdown'),
  path.resolve(CONFORMANCE_DIR, '..', '..', 'pokemon-showdown'),
];

function showdownDir() {
  const e = explicit();
  if (e) {
    if (isBuilt(e.dir)) return e.dir;
    throw new Error(`${e.source}=${e.dir} has no dist/sim; run \`node build\` in that checkout`);
  }
  for (const d of DEFAULTS) if (isBuilt(d)) return d;
  throw new Error(`pokemon-showdown not found (tried ${DEFAULTS.join(', ')}); set SHOWDOWN_DIR or showdown_dir in ${CONFIG_PATH}`);
}

function showdownSim() {
  return path.join(showdownDir(), 'dist', 'sim');
}

module.exports = { showdownDir, showdownSim };
