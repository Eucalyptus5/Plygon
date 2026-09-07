'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

// Reference: the previous whole-file implementation, verbatim.
function readTrailingStatsOld(statsPath, n) {
  if (!fs.existsSync(statsPath)) return [];
  const raw = fs.readFileSync(statsPath, 'utf8');
  const lines = raw.split('\n').filter((l) => l.length > 0);
  const trail = lines.slice(-n);
  const parsed = [];
  for (let i = 0; i < trail.length; i++) {
    try { parsed.push(JSON.parse(trail[i])); }
    catch (_) { if (i === trail.length - 1) continue; }
  }
  return parsed;
}

function withStatsPath(p, fn) {
  const prev = process.env.FUZZ_STATS_PATH;
  process.env.FUZZ_STATS_PATH = p;
  delete require.cache[require.resolve('../fuzz.js')];
  const { readTrailingStats } = require('../fuzz.js');
  try { return fn(readTrailingStats); }
  finally {
    if (prev === undefined) delete process.env.FUZZ_STATS_PATH;
    else process.env.FUZZ_STATS_PATH = prev;
    delete require.cache[require.resolve('../fuzz.js')];
  }
}

const PROD_STATS = path.join(__dirname, '..', 'stats.jsonl');

test('tail-read matches whole-file read on real stats.jsonl across window sizes', () => {
  if (!fs.existsSync(PROD_STATS)) { console.log('skip: no production stats.jsonl'); return; }
  withStatsPath(PROD_STATS, (readTrailingStats) => {
    for (const n of [50, 1000, 10000, 25000]) {
      const got = readTrailingStats(n);
      const want = readTrailingStatsOld(PROD_STATS, n);
      assert.deepStrictEqual(got, want, `mismatch at n=${n}: got ${got.length} want ${want.length}`);
    }
  });
});

test('tail-read tolerates truncated final line and corrupt mid-window line', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tailread-'));
  const p = path.join(dir, 'stats.jsonl');
  try {
    const rows = [];
    for (let i = 0; i < 200; i++) rows.push(JSON.stringify({ seed: i, oracle_coverage: ['x'] }));
    rows[100] = '{not valid json';                 // corrupt mid-window line
    let body = rows.slice(0, 199).join('\n') + '\n' + '{"seed":199,"oracle_';
    fs.writeFileSync(p, body);
    withStatsPath(p, (readTrailingStats) => {
      for (const n of [10, 50, 250]) {
        assert.deepStrictEqual(readTrailingStats(n), readTrailingStatsOld(p, n), `n=${n}`);
      }
    });
  } finally { fs.rmSync(dir, { recursive: true, force: true }); }
});

test('adaptive doubling: window larger than initial K still recovers n lines', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tailread-big-'));
  const p = path.join(dir, 'stats.jsonl');
  try {
    // The 2 KB pad makes the file multi-megabyte, so the read is sized by n and not the 1 MiB floor.
    const pad = 'q'.repeat(2000);
    const rows = [];
    for (let i = 0; i < 2000; i++) rows.push(JSON.stringify({ seed: i, pad }));
    fs.writeFileSync(p, rows.join('\n') + '\n');
    withStatsPath(p, (readTrailingStats) => {
      const got = readTrailingStats(1500);
      const want = readTrailingStatsOld(p, 1500);
      assert.strictEqual(got.length, 1500);
      assert.deepStrictEqual(got, want);
    });
  } finally { fs.rmSync(dir, { recursive: true, force: true }); }
});

test('missing / empty file returns []', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tailread-empty-'));
  try {
    const missing = path.join(dir, 'nope.jsonl');
    withStatsPath(missing, (rt) => assert.deepStrictEqual(rt(10), []));
    const empty = path.join(dir, 'empty.jsonl');
    fs.writeFileSync(empty, '');
    withStatsPath(empty, (rt) => assert.deepStrictEqual(rt(10), []));
  } finally { fs.rmSync(dir, { recursive: true, force: true }); }
});
