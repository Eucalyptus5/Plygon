'use strict';

// Without started_at the PID-reuse check fails open and steals the lock from a live holder.

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

function lockPath(inboxDir) {
  return path.join(inboxDir, '.fuzz.lock');
}

function spawnCapture(cmd, args) {
  const r = spawnSync(cmd, args, { stdio: ['ignore', 'pipe', 'ignore'], shell: false });
  if (r.status !== 0 || r.signal) return null;
  const out = (r.stdout || '').toString().trim();
  return out.length ? out : null;
}

function processStartTimeSeconds(pid) {
  // Refuse non-integer pids so a lockfile cannot subvert the spawnSync arg vector.
  if (!Number.isInteger(pid) || pid <= 0) return null;
  try {
    if (process.platform === 'linux') {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
      const fields = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
      const starttimeTicks = parseInt(fields[19], 10);
      const clkTckRaw = spawnCapture('getconf', ['CLK_TCK']);
      const clkTck = parseInt(clkTckRaw, 10) || 100;
      const uptime = parseFloat(fs.readFileSync('/proc/uptime', 'utf8').split(' ')[0]);
      const bootSec = (Date.now() / 1000) - uptime;
      return bootSec + starttimeTicks / clkTck;
    }
    const out = spawnCapture('ps', ['-o', 'lstart=', '-p', String(pid)]);
    if (!out) return null;
    const t = Date.parse(out);
    return Number.isFinite(t) ? t / 1000 : null;
  } catch (_) {
    return null;
  }
}

function pidExists(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; } catch (e) { return e.code === 'EPERM'; }
}

function inspect({ inboxDir }) {
  const p = lockPath(inboxDir);
  if (!fs.existsSync(p)) return null;
  try { return JSON.parse(fs.readFileSync(p, 'utf8')); }
  catch (_) { return null; }
}

function acquire({ inboxDir, holder }) {
  if (!inboxDir) throw new Error('lock.acquire: inboxDir required');
  if (typeof holder !== 'string' || holder.length === 0) {
    throw new Error('lock.acquire: holder (non-empty string) required');
  }
  fs.mkdirSync(inboxDir, { recursive: true });
  const p = lockPath(inboxDir);
  if (fs.existsSync(p)) {
    let prior;
    try { prior = JSON.parse(fs.readFileSync(p, 'utf8')); }
    catch (e) {
      const err = new Error(`lock.acquire: stale unparseable .fuzz.lock — refuse to clobber: ${e.message}`);
      err.code = 'LOCK_UNPARSEABLE';
      throw err;
    }
    if (prior.hostname !== os.hostname()) {
      const err = new Error(`lock.acquire: .fuzz.lock from different host — refuse: ${JSON.stringify(prior)}`);
      err.code = 'LOCK_FOREIGN_HOST';
      throw err;
    }
    const startedAtSec = Date.parse(prior.started_at) / 1000;
    if (pidExists(prior.pid)) {
      const procStart = processStartTimeSeconds(prior.pid);
      if (procStart !== null && procStart >= startedAtSec - 1) {
        const err = new Error(`lock.acquire: live writer holding .fuzz.lock — refuse: ${JSON.stringify(prior)}`);
        err.code = 'LOCK_LIVE_HOLDER';
        throw err;
      }
    }
    try { fs.unlinkSync(p); } catch (_) { /* race-tolerant */ }
  }
  const payload = {
    pid: process.pid,
    started_at: new Date().toISOString(),
    hostname: os.hostname(),
    holder,
  };
  fs.writeFileSync(p, JSON.stringify(payload));
  // 'exit' fires synchronously; async drain happens via installSignalHandlers.
  process.on('exit', () => release({ inboxDir }));
  return payload;
}

function release({ inboxDir }) {
  try { fs.unlinkSync(lockPath(inboxDir)); } catch (_) {}
}

function installSignalHandlers({ inboxDir, drain }) {
  const doDrain = typeof drain === 'function' ? drain : async () => {};
  process.on('SIGINT', async () => {
    try { await doDrain(); } catch (_) {}
    release({ inboxDir });
    process.exit(130);
  });
  process.on('SIGTERM', async () => {
    try { await doDrain(); } catch (_) {}
    release({ inboxDir });
    process.exit(143);
  });
}

module.exports = {
  acquire,
  release,
  installSignalHandlers,
  inspect,
  lockPath,
};
