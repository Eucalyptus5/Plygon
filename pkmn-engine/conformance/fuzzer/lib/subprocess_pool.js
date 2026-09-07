'use strict';

// The child speaks NDJSON and must echo the request's __req_id on its response line.

const {spawn, spawnSync} = require('child_process');
const readline = require('readline');
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const STDERR_RING_CAP = 8 * 1024;          // 8 KiB tail-ring per pool
const RSS_SAMPLE_INTERVAL_MS = 1000;       // ≥1 s cadence (ps shell-out cost)
const RESTART_WINDOW_MS = 60_000;          // restart-loop guard window
const RESTART_LIMIT = 5;                   // >5 restarts inside window → fatal
const SHUTDOWN_GRACE_MS = 5_000;           // SIGTERM → SIGKILL grace
const PROTOCOL_DESYNC_DIR = path.join(
  __dirname, '..', 'inbox', 'protocol_desync'
);
const HEARTBEAT_INTERVAL_MS = parseInt(process.env.POOL_HEARTBEAT_MS || '0', 10);
const HEARTBEAT_STALL_MS = parseInt(process.env.POOL_HEARTBEAT_STALL_MS || '5000', 10);

class SubprocessPool {
  constructor({command, args = [], label, restartLimit = RESTART_LIMIT,
               recycleAfter = 1000, rssCapMb,
               kind = 'generic'}) {
    this.command = command;
    this.args = args;
    this.label = label || command;
    this.restartLimit = restartLimit;
    this.recycleAfter = recycleAfter;
    this.rssCapMb = rssCapMb;
    this.kind = kind;                       // 'engine' | 'showdown' | 'generic'

    this.child = null;
    this.rl = null;
    this.spawnEpoch = '';
    this.counter = 0;
    this.poolCallCount = 0;
    this.recyclePending = false;
    this.rssExceeded = false;

    this.stderrRing = [];
    this.stderrBytes = 0;

    this.restartTimestamps = [];
    this.shuttingDown = false;
    this.rssTimer = null;

    this._inflight = null;                  // {reqId, resolve, reject, timer}
    this._lock = Promise.resolve();         // simple chained-promise serializer

    this._inflightMeta = null;              // {callId, sendTs, queueDepthAtSend, lockHolderAtSend}
    this._lockHolder = null;                 // callId currently holding _lock
    this._lockQueueDepth = 0;                // pending acquires (incl. holder)
    this._lastHeartbeatLog = 0;              // ms — throttle stderr emit per stuck call
    this._heartbeatTimer = null;
    if (HEARTBEAT_INTERVAL_MS > 0) {
      this._heartbeatTimer = setInterval(
        () => this._heartbeat(), HEARTBEAT_INTERVAL_MS
      );
      if (this._heartbeatTimer.unref) this._heartbeatTimer.unref();
    }

    this._spawn();
  }

  _heartbeat() {
    if (this.shuttingDown) return;
    const m = this._inflightMeta;
    if (!m) return;
    const now = Date.now();
    const elapsed = now - m.sendTs;
    if (elapsed < HEARTBEAT_STALL_MS) return;
    if (now - this._lastHeartbeatLog < HEARTBEAT_INTERVAL_MS) return;
    this._lastHeartbeatLog = now;
    process.stderr.write(
      `pool=${this.label} waiting_on=${m.callId} elapsed=${elapsed}ms ` +
      `queue_depth=${m.queueDepthAtSend} lock_holder=${m.lockHolderAtSend} ` +
      `child_pid=${this.child && this.child.pid} ` +
      `stdin_writable=${!!(this.child && this.child.stdin && this.child.stdin.writable)}\n`
    );
  }

  _spawn() {
    if (this.shuttingDown) return;
    this.spawnEpoch =
      Date.now().toString(36) + crypto.randomBytes(4).toString('hex');
    this.counter = 0;
    this.poolCallCount = 0;
    this.recyclePending = false;
    this.rssExceeded = false;
    this.stderrRing = [];
    this.stderrBytes = 0;

    const child = spawn(this.command, this.args, {
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    this.child = child;

    child.stderr.on('data', (chunk) => {
      this.stderrRing.push(chunk);
      this.stderrBytes += chunk.length;
      while (this.stderrBytes > STDERR_RING_CAP && this.stderrRing.length > 1) {
        const dropped = this.stderrRing.shift();
        this.stderrBytes -= dropped.length;
      }
    });

    // Multi-MiB lines only drain safely because one request is in flight per pool; pools sharing an event loop would break that.
    const rl = readline.createInterface({input: child.stdout, terminal: false});
    this.rl = rl;
    rl.on('line', (line) => this._onLine(line));

    child.on('exit', (code, signal) => this._onExit(code, signal));
    child.on('error', (err) => this._onExit(null, null, err));

    if (this.rssCapMb) this._startRssWatchdog();
  }

  _onLine(line) {
    const pending = this._inflight;
    if (!pending) {
      this._dumpDesync('stray-line', line);
      this._respawn('stray-line');
      return;
    }
    this._completePending(pending, line);
  }

  _completePending(pending, line) {
    if (pending.timer) clearTimeout(pending.timer);
    this._inflight = null;

    if (line === '') {
      this._dumpDesync('blank-line', line);
      const stderrSnap = this._snapshotStderr();
      this._respawn('blank-line');
      pending.reject(this._failure({
        status: 1, signal: null, stdout: line, stderr: stderrSnap,
        respawned: true, reason: 'blank_line',
      }));
      return;
    }

    let resp;
    try {
      resp = JSON.parse(line);
    } catch (e) {
      // A broken body still addressed to the pending request is a result, not a desync.
      const m = line.match(/"__req_id"\s*:\s*("([^"\\]|\\.)*"|null|\d+)/);
      let extractedId = null;
      if (m) {
        try { extractedId = JSON.parse(m[1]); } catch (_) { /* ignore */ }
      }
      if (extractedId !== null && String(extractedId) === pending.reqId) {
        const stderrSnap = this._snapshotStderr();
        pending.resolve({
          status: 0, signal: null, stdout: line, stderr: stderrSnap,
          respawned: false,
        });
        return;
      }
      this._dumpDesync('parse-fail', line);
      const stderrSnap = this._snapshotStderr();
      this._respawn('parse-fail');
      pending.reject(this._failure({
        status: 1, signal: null, stdout: line, stderr: stderrSnap,
        respawned: true, reason: 'parse_fail',
      }));
      return;
    }

    const respId = resp.__req_id;
    if (respId === undefined || respId === null ||
        String(respId) !== pending.reqId) {
      this._dumpDesync('reqid-mismatch', line);
      const stderrSnap = this._snapshotStderr();
      this._respawn('reqid-mismatch');
      pending.reject(this._failure({
        status: 1, signal: null, stdout: line, stderr: stderrSnap,
        respawned: true, reason: 'reqid_mismatch',
      }));
      return;
    }

    // Engine failures are reshaped as a nonzero exit so routeEngineFailure still classifies them.
    if (this.kind === 'engine' && resp.success === false) {
      const stderrSnap = this._snapshotStderr();
      pending.resolve({
        status: 1, signal: null, stdout: line,
        stderr: resp.error || stderrSnap, respawned: false,
      });
      return;
    }

    delete resp.__req_id;
    pending.resolve({
      status: 0, signal: null, stdout: line, stderr: '',
      response: resp, respawned: false,
    });
  }

  _onExit(code, signal, err) {
    const pending = this._inflight;
    this._inflight = null;
    if (this.rssTimer) {
      clearInterval(this.rssTimer);
      this.rssTimer = null;
    }
    if (this.shuttingDown) return;

    if (pending) {
      if (pending.timer) clearTimeout(pending.timer);
      const stderrSnap = this._snapshotStderr();
      pending.reject(this._failure({
        status: code === null ? 1 : code,
        signal: signal || null,
        stdout: '', stderr: stderrSnap,
        respawned: true, reason: 'unexpected_exit',
        spawnError: err ? err.message : null,
      }));
    }
    this._respawn('exit');
  }

  _respawn(why) {
    if (this.shuttingDown) return;
    this.restartTimestamps.push(Date.now());
    const cutoff = Date.now() - RESTART_WINDOW_MS;
    this.restartTimestamps = this.restartTimestamps.filter((t) => t > cutoff);
    if (this.restartTimestamps.length > this.restartLimit) {
      const e = new Error(
        `${this.label}: restart limit exceeded ` +
        `(${this.restartTimestamps.length} restarts in ` +
        `${RESTART_WINDOW_MS / 1000}s; last reason: ${why})`
      );
      e.poolFatal = true;
      throw e;
    }
    this._tearDownChild();
    this._spawn();
  }

  _tearDownChild() {
    if (this.rl) {
      this.rl.removeAllListeners('line');
      try { this.rl.close(); } catch (_) { /* ignore */ }
    }
    if (this.child) {
      this.child.stdout.removeAllListeners('data');
      this.child.stderr.removeAllListeners('data');
      this.child.removeAllListeners('exit');
      this.child.removeAllListeners('error');
      try { this.child.kill('SIGKILL'); } catch (_) { /* ignore */ }
    }
    this.rl = null;
    this.child = null;
  }

  _startRssWatchdog() {
    if (this.rssTimer) clearInterval(this.rssTimer);
    this.rssTimer = setInterval(() => {
      if (!this.child || !this.child.pid) return;
      const out = spawnSync('ps', ['-o', 'rss=', '-p', String(this.child.pid)]);
      if (out.status !== 0) return;
      const kb = parseInt(String(out.stdout).trim(), 10);
      if (!Number.isFinite(kb)) return;
      const mb = Math.floor(kb / 1024);
      if (mb > this.rssCapMb) this.rssExceeded = true;
    }, RSS_SAMPLE_INTERVAL_MS);
    if (this.rssTimer.unref) this.rssTimer.unref();
  }

  async runScenario(scenarioObj, timeoutMs) {
    const queueDepthAtSend = this._lockQueueDepth;
    this._lockQueueDepth++;
    const release = await this._acquire();
    const reqId = `${this.spawnEpoch}-${this.counter++}`;
    const lockHolderAtSend = this._lockHolder;
    this._lockHolder = reqId;
    try {
      const payload = {...scenarioObj, __req_id: reqId};
      const wireLine = JSON.stringify(payload) + '\n';

      this._inflightMeta = {
        callId: reqId,
        sendTs: Date.now(),
        queueDepthAtSend,
        lockHolderAtSend,
      };

      const result = await new Promise((resolve, reject) => {
        const timer = timeoutMs > 0
          ? setTimeout(() => {
              this._inflight = null;
              const stderrSnap = this._snapshotStderr();
              this._respawn('timeout');
              reject(this._failure({
                status: 1, signal: 'SIGTERM',
                stdout: '', stderr: stderrSnap,
                respawned: true, reason: 'timeout',
              }));
            }, timeoutMs)
          : null;
        this._inflight = {reqId, resolve, reject, timer};
        if (!this.child || !this.child.stdin.writable) {
          if (timer) clearTimeout(timer);
          this._inflight = null;
          const stderrSnap = this._snapshotStderr();
          reject(this._failure({
            status: 1, signal: null, stdout: '', stderr: stderrSnap,
            respawned: true, reason: 'no_child',
          }));
          return;
        }
        this.child.stdin.write(wireLine, (err) => {
          if (err) {
            if (timer) clearTimeout(timer);
            this._inflight = null;
            const stderrSnap = this._snapshotStderr();
            this._respawn('write-error');
            reject(this._failure({
              status: 1, signal: null, stdout: '',
              stderr: err.message + '\n' + stderrSnap,
              respawned: true, reason: 'write_error',
            }));
          }
        });
      });

      this.poolCallCount++;
      if (this.poolCallCount >= this.recycleAfter || this.rssExceeded) {
        this.recyclePending = true;
      }
      return result;
    } finally {
      this._inflightMeta = null;
      this._lockHolder = null;
      this._lockQueueDepth--;
      release();
    }
  }

  async recycleNow() {
    if (this.shuttingDown) return;
    if (!this.child) { this._spawn(); return; }
    const child = this.child;
    this._tearDownChild();
    try { child.kill('SIGTERM'); } catch (_) { /* ignore */ }
    await this._waitForExit(child, SHUTDOWN_GRACE_MS);
    try { child.kill('SIGKILL'); } catch (_) { /* ignore */ }
    this._spawn();
  }

  async shutdown() {
    if (this.shuttingDown) return;
    this.shuttingDown = true;
    if (this.rssTimer) {
      clearInterval(this.rssTimer);
      this.rssTimer = null;
    }
    if (this._heartbeatTimer) {
      clearInterval(this._heartbeatTimer);
      this._heartbeatTimer = null;
    }
    const child = this.child;
    this._tearDownChild();
    if (!child) return;
    try { child.stdin.end(); } catch (_) { /* ignore */ }
    try { child.kill('SIGTERM'); } catch (_) { /* ignore */ }
    await this._waitForExit(child, SHUTDOWN_GRACE_MS);
    try { child.kill('SIGKILL'); } catch (_) { /* ignore */ }
  }

  _acquire() {
    let release;
    const wait = new Promise((resolve) => { release = resolve; });
    const prev = this._lock;
    this._lock = wait;
    return prev.then(() => release);
  }

  _waitForExit(child, ms) {
    return new Promise((resolve) => {
      let done = false;
      const finish = () => { if (!done) { done = true; resolve(); } };
      child.once('exit', finish);
      setTimeout(finish, ms).unref?.();
    });
  }

  _snapshotStderr() {
    if (!this.stderrRing.length) return '';
    return Buffer.concat(this.stderrRing).toString('utf-8');
  }

  _failure(obj) {
    const e = new Error(obj.reason || 'pool_failure');
    Object.assign(e, obj);
    return e;
  }

  _dumpDesync(reason, line) {
    try {
      fs.mkdirSync(PROTOCOL_DESYNC_DIR, {recursive: true});
      const stamp = new Date().toISOString().replace(/[:.]/g, '-');
      const file = path.join(
        PROTOCOL_DESYNC_DIR,
        `${stamp}-${this.label}-${reason}.txt`
      );
      const stderrSnap = this._snapshotStderr();
      fs.writeFileSync(
        file,
        `pool=${this.label}\nreason=${reason}\nspawnEpoch=${this.spawnEpoch}\n` +
        `pendingReqId=${this._inflight ? this._inflight.reqId : 'none'}\n` +
        `--- raw line ---\n${line}\n--- stderr tail ---\n${stderrSnap}\n`
      );
    } catch (_) { /* best-effort */ }
  }
}

module.exports = {SubprocessPool};
