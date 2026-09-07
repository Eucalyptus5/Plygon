'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');

const { signatureMentionsSplash, SPLASH_MOVE_ID } = require('../lib/splash_signature.js');

test('SPLASH_MOVE_ID is 150', () => {
  assert.strictEqual(SPLASH_MOVE_ID, 150);
});

test('returns false on empty / null signature', () => {
  assert.strictEqual(signatureMentionsSplash(null), false);
  assert.strictEqual(signatureMentionsSplash(undefined), false);
  assert.strictEqual(signatureMentionsSplash([]), false);
  assert.strictEqual(signatureMentionsSplash(new Set()), false);
});

test('detects array-tuple shape: tuple[4] === 150', () => {
  const sig = [
    [0, { p1: [25, 9, 100, 0, 150, true], p2: [25, 9, 100, 0, 33, true] }, 'team.is_fainted'],
  ];
  assert.strictEqual(signatureMentionsSplash(sig), true);
});

test('detects object-tuple shape: effective_move_id_if_moved === 150', () => {
  const sig = [
    {
      entity_id_set_at_turn: {
        p1: { species_id: 25, ability_id: 9, item_id: 100, action_byte: 0, effective_move_id_if_moved: 150, did_move: true },
        p2: { species_id: 25, ability_id: 9, item_id: 100, action_byte: 0, effective_move_id_if_moved: 33, did_move: true },
      },
      category: 'team.is_fainted',
    },
  ];
  assert.strictEqual(signatureMentionsSplash(sig), true);
});

test('returns false when no entity tuple carries 150', () => {
  const sig = [
    [0, { p1: [25, 9, 100, 0, 33, true], p2: [25, 9, 100, 0, 33, true] }, 'team.is_fainted'],
  ];
  assert.strictEqual(signatureMentionsSplash(sig), false);
});

// Identity check: production must resolve to the module these tests exercise.
test('fuzz.js imports ./lib/splash_signature.js', () => {
  const fuzzPath = path.resolve(__dirname, '..', 'fuzz.js');
  const fuzzSrc = fs.readFileSync(fuzzPath, 'utf8');
  assert.ok(/require\(['"]\.\/lib\/splash_signature\.js['"]\)/.test(fuzzSrc),
    'fuzz.js: expected import of ./lib/splash_signature.js');
});

test('fuzz.js: SUPPRESSED_HIGH + signatureMentionsSplash branch present', () => {
  const fuzzPath = path.resolve(__dirname, '..', 'fuzz.js');
  const src = fs.readFileSync(fuzzPath, 'utf8');
  assert.ok(/INBOX_SUPPRESSED_SPLASH/.test(src),
    'fuzz.js: INBOX_SUPPRESSED_SPLASH constant missing');
  assert.ok(/CLASSIFICATIONS\.SUPPRESSED_HIGH[\s\S]{0,80}signatureMentionsSplash/.test(src),
    'fuzz.js: SUPPRESSED_HIGH + signatureMentionsSplash routing branch missing');
  assert.ok(/ensureDir\(INBOX_SUPPRESSED_SPLASH\)/.test(src),
    'fuzz.js: ensureDir(INBOX_SUPPRESSED_SPLASH) call missing');
});
