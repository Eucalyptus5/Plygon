'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');

const FUZZ_JS = path.resolve(__dirname, '..', 'fuzz.js');

test('fuzz.js: payload literal carries pid, started_at, hostname, holder', () => {
  const src = fs.readFileSync(FUZZ_JS, 'utf8');
  const m = src.match(/const\s+payload\s*=\s*\{([\s\S]*?)\};/);
  assert.ok(m, 'fuzz.js: no `const payload = { ... };` literal found');
  const body = m[1];
  for (const key of ['pid', 'started_at', 'hostname', 'holder']) {
    assert.ok(new RegExp(`\\b${key}\\s*:`).test(body), `fuzz.js payload missing key: ${key}`);
  }
});

test('fuzz.js: prior.started_at parse reference survives (PID-reuse gate is load-bearing)', () => {
  const src = fs.readFileSync(FUZZ_JS, 'utf8');
  assert.ok(/prior\.started_at/.test(src),
    'fuzz.js: prior.started_at reference removed — PID-reuse staleness gate degraded to "trust any same-host PID"');
});
