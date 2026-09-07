'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const lock = require('../lib/lock.js');

function mkTmp() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lock-test-'));
  return dir;
}

test('acquire writes the canonical 4-key payload', () => {
  const dir = mkTmp();
  try {
    const payload = lock.acquire({ inboxDir: dir, holder: 'lock.test.js' });
    assert.strictEqual(payload.holder, 'lock.test.js');
    assert.strictEqual(payload.pid, process.pid);
    assert.strictEqual(payload.hostname, os.hostname());
    assert.ok(payload.started_at);
    const disk = JSON.parse(fs.readFileSync(lock.lockPath(dir), 'utf8'));
    for (const k of ['pid', 'started_at', 'hostname', 'holder']) {
      assert.ok(Object.prototype.hasOwnProperty.call(disk, k), `missing ${k}`);
    }
  } finally {
    lock.release({ inboxDir: dir });
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('release is idempotent', () => {
  const dir = mkTmp();
  try {
    lock.acquire({ inboxDir: dir, holder: 'h' });
    lock.release({ inboxDir: dir });
    lock.release({ inboxDir: dir });
    assert.ok(!fs.existsSync(lock.lockPath(dir)));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('refuses on unparseable lockfile', () => {
  const dir = mkTmp();
  try {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(lock.lockPath(dir), '{this is not json');
    assert.throws(() => lock.acquire({ inboxDir: dir, holder: 'h' }), /unparseable/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('refuses on foreign-host lock', () => {
  const dir = mkTmp();
  try {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(lock.lockPath(dir), JSON.stringify({
      pid: 9999999, hostname: 'definitely-not-this-host', started_at: new Date().toISOString(), holder: 'fake',
    }));
    assert.throws(() => lock.acquire({ inboxDir: dir, holder: 'h' }), /different host/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('refuses on live-holder lock (process.pid itself)', () => {
  const dir = mkTmp();
  try {
    fs.mkdirSync(dir, { recursive: true });
    // Our own PID plus a just-written started_at reads as a live writer, not a reused PID.
    fs.writeFileSync(lock.lockPath(dir), JSON.stringify({
      pid: process.pid, hostname: os.hostname(), started_at: new Date().toISOString(), holder: 'fake',
    }));
    assert.throws(() => lock.acquire({ inboxDir: dir, holder: 'h' }), /live writer/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('reclaims stale-PID lockfile', () => {
  const dir = mkTmp();
  try {
    fs.mkdirSync(dir, { recursive: true });
    // 2147483646 is never a live PID, so pidExists fails and the row is reclaimed as stale.
    fs.writeFileSync(lock.lockPath(dir), JSON.stringify({
      pid: 2147483646, hostname: os.hostname(), started_at: new Date().toISOString(), holder: 'fake',
    }));
    const payload = lock.acquire({ inboxDir: dir, holder: 'h' });
    assert.strictEqual(payload.pid, process.pid);
    lock.release({ inboxDir: dir });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('inspect returns payload when present, null otherwise', () => {
  const dir = mkTmp();
  try {
    assert.strictEqual(lock.inspect({ inboxDir: dir }), null);
    lock.acquire({ inboxDir: dir, holder: 'h' });
    const p = lock.inspect({ inboxDir: dir });
    assert.ok(p && p.holder === 'h');
    lock.release({ inboxDir: dir });
    assert.strictEqual(lock.inspect({ inboxDir: dir }), null);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('inspect returns null on malformed payload (does NOT throw)', () => {
  const dir = mkTmp();
  try {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(lock.lockPath(dir), '{bad json');
    assert.strictEqual(lock.inspect({ inboxDir: dir }), null);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
