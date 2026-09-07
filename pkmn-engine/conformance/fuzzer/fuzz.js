#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const { spawnSync, execSync } = require('child_process');

const {
  CLASSIFICATIONS,
  REQUIRED_CATEGORIES,
  SIGNATURE_CATEGORIES,
  ORACLE_SCOPE_ALLOWLIST,
  path_to_category,
} = require('./constants.js');

const generate = require('./generate.js');
const minimize = require('./minimize.js');
const suppress = require('./suppress.js');

const { compareResults, loadFormeMap } = require('../harness/compare_results.js');

const halt = require('./lib/halt_flag.js');

let FORME_MAP = {};

const PHASE_TIMING = !!process.env.FUZZ_PHASE_TIMING;
const PHASE_STATS = PHASE_TIMING ? Object.create(null) : null;

function recordPhase(name, ms) {
  const s = PHASE_STATS[name] || (PHASE_STATS[name] = { count: 0, ms: 0 });
  s.count++; s.ms += ms;
}

function timed(name, thunk) {
  if (!PHASE_TIMING) return thunk();
  const t = Date.now();
  const r = thunk();
  if (r && typeof r.then === 'function') {
    return r.finally(() => recordPhase(name, Date.now() - t));
  }
  recordPhase(name, Date.now() - t);
  return r;
}

function emitPhaseTimingSummary(totalScenarios, wallMs) {
  if (!PHASE_TIMING) return;
  const rows = Object.keys(PHASE_STATS).map((name) => {
    const s = PHASE_STATS[name];
    return { name, count: s.count, ms: s.ms, mean: s.ms / Math.max(1, s.count) };
  }).sort((a, b) => b.ms - a.ms);
  const totalPhaseMs = rows.reduce((a, r) => a + r.ms, 0) || 1;
  console.error('\nfuzz.js FUZZ_PHASE_TIMING summary:');
  console.error('  phase           count      total_ms    mean_ms   share');
  for (const r of rows) {
    console.error(
      `  ${r.name.padEnd(14)} ${String(r.count).padStart(7)} ${r.ms.toFixed(0).padStart(12)} `
      + `${r.mean.toFixed(2).padStart(9)} ${(100 * r.ms / totalPhaseMs).toFixed(1).padStart(6)}%`,
    );
  }
  const sps = totalScenarios / (Math.max(1, wallMs) / 1000);
  console.error(`  → ${totalScenarios} scenarios in ${(wallMs / 1000).toFixed(1)}s = `
    + `${sps.toFixed(2)} scenarios/s, ${(wallMs / Math.max(1, totalScenarios)).toFixed(1)} ms/scenario\n`);
}

let KNOWN_BUGS_CACHE = {};

// Each pool serves one in-flight request, so concurrency is the number of pairs.
let ENGINE_POOL = null;
let SHOWDOWN_POOL = null;
const POOL_PAIRS = [];

const {
  enumerateTeamPaths,
  enumerateActivePaths,
  enumerateSideConditionPaths,
  enumerateFieldPaths,
} = require('../harness/lib/snapshot_paths.js');

const FUZZER_DIR = __dirname;
const CONFORMANCE_DIR = path.resolve(FUZZER_DIR, '..');
const HARNESS_DIR = path.join(CONFORMANCE_DIR, 'harness');
const SHOWDOWN_RUNNER = path.join(HARNESS_DIR, 'showdown_runner.js');
const ENGINE_BIN = path.join(HARNESS_DIR, 'run_scenario', 'target', 'debug', 'run_scenario');
const KNOWN_BUGS_PATH = path.join(FUZZER_DIR, 'known_divergences.json');
const FORME_MAP_PATH = path.join(CONFORMANCE_DIR, 'id_maps', 'engine_forme_to_showdown.json');

// The env overrides let isolated verification runs avoid the production inbox and stats.
const INBOX_DIR = process.env.FUZZ_INBOX_DIR
  ? path.resolve(process.env.FUZZ_INBOX_DIR)
  : path.join(FUZZER_DIR, 'inbox');
const INBOX_BUGS = path.join(INBOX_DIR, 'bugs');
const INBOX_SUPPRESSED = path.join(INBOX_DIR, 'suppressed');
const INBOX_SUPPRESSED_SPLASH = path.join(INBOX_DIR, 'suppressed_splash');
const INBOX_REJECTED = path.join(INBOX_DIR, 'rejected');
const INBOX_SPLASH = path.join(INBOX_DIR, 'splash');
const LOCK_PATH = path.join(INBOX_DIR, '.fuzz.lock');

const { signatureMentionsSplash } = require('./lib/splash_signature.js');

const STATS_PATH = process.env.FUZZ_STATS_PATH
  ? path.resolve(process.env.FUZZ_STATS_PATH)
  : path.join(FUZZER_DIR, 'stats.jsonl');
const CONFIG_PATH = process.env.FUZZ_CONFIG_PATH
  ? path.resolve(process.env.FUZZ_CONFIG_PATH)
  : path.join(FUZZER_DIR, 'config.json');
const ORACLE_SCOPE_PATH = path.join(FUZZER_DIR, 'oracle_scope.md');
const HARNESS_ISSUE_ORACLE = path.join(FUZZER_DIR, 'HARNESS_ISSUE_ORACLE.md');

function parseArgs(argv) {
  const out = { maxScenarios: null, seed: null, fixture: null, unitTest: null };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--max-scenarios') {
      out.maxScenarios = parseInt(argv[++i], 10);
    } else if (a === '--seed') {
      const raw = argv[++i];
      out.seed = raw.startsWith('0x') || raw.startsWith('0X')
        ? parseInt(raw.slice(2), 16)
        : parseInt(raw, 10);
    } else if (a === '--fixture') {
      out.fixture = argv[++i];
    } else if (a === '--unit-test') {
      out.unitTest = argv[++i];
    } else {
      console.error(`fuzz.js: unknown flag ${a}`);
      process.exit(2);
    }
  }
  return out;
}

const ARGV = parseArgs(process.argv);

if (ARGV.fixture && (ARGV.maxScenarios !== null || ARGV.seed !== null)) {
  console.error('fuzz.js: --fixture is mutually exclusive with --max-scenarios and --seed');
  process.exit(2);
}

function processStartTimeSeconds(pid) {
  try {
    if (process.platform === 'linux') {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
      // field 22 is starttime in clock ticks since boot
      const fields = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
      const starttimeTicks = parseInt(fields[19], 10); // 0-based after the close-paren split
      const clkTck = parseInt(execSync('getconf CLK_TCK').toString().trim(), 10) || 100;
      const uptime = parseFloat(fs.readFileSync('/proc/uptime', 'utf8').split(' ')[0]);
      const bootSec = (Date.now() / 1000) - uptime;
      return bootSec + starttimeTicks / clkTck;
    }
    const out = execSync(`ps -o lstart= -p ${pid}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
    if (!out) return null;
    const t = Date.parse(out);
    return Number.isFinite(t) ? t / 1000 : null;
  } catch (_) {
    return null;
  }
}

function pidExists(pid) {
  try { process.kill(pid, 0); return true; } catch (e) { return e.code === 'EPERM'; }
}

function acquireLock() {
  fs.mkdirSync(INBOX_DIR, { recursive: true });
  if (fs.existsSync(LOCK_PATH)) {
    let prior;
    try { prior = JSON.parse(fs.readFileSync(LOCK_PATH, 'utf8')); }
    catch (e) {
      console.error(`fuzz.js: stale unparseable .fuzz.lock — refuse to clobber: ${e.message}`);
      process.exit(2);
    }
    const sameHost = prior.hostname === os.hostname();
    if (!sameHost) {
      console.error(`fuzz.js: .fuzz.lock from different host — refuse: ${JSON.stringify(prior)}`);
      process.exit(2);
    }
    const startedAtSec = Date.parse(prior.started_at) / 1000;
    if (pidExists(prior.pid)) {
      const procStart = processStartTimeSeconds(prior.pid);
      if (procStart !== null && procStart >= startedAtSec - 1) {
        console.error(`fuzz.js: live writer holding .fuzz.lock — refuse: ${JSON.stringify(prior)}`);
        process.exit(2);
      }
    }
    try { fs.unlinkSync(LOCK_PATH); } catch (_) { /* race-tolerant */ }
  }
  const payload = {
    pid: process.pid,
    started_at: new Date().toISOString(),
    hostname: os.hostname(),
    holder: 'fuzz.js',
  };
  fs.writeFileSync(LOCK_PATH, JSON.stringify(payload));
  // The 'exit' event is synchronous, so pool drain happens in shutdownAndExit and the signal handlers.
  process.on('exit', releaseLock);
  process.on('SIGINT', async () => {
    halt.halt('SIGINT');
    await drainPools();
    releaseLock();
    process.exit(130);
  });
  process.on('SIGTERM', async () => {
    halt.halt('SIGTERM');
    await drainPools();
    releaseLock();
    process.exit(143);
  });
}

function releaseLock() {
  try { fs.unlinkSync(LOCK_PATH); } catch (_) {}
}

async function drainPools() {
  const tasks = [];
  for (const pair of POOL_PAIRS) {
    if (pair.engine) tasks.push(pair.engine.shutdown());
    if (pair.showdown) tasks.push(pair.showdown.shutdown());
  }
  await Promise.allSettled(tasks);
}

async function shutdownAndExit(code) {
  // Halt before draining so in-flight workers stop scheduling.
  halt.halt(`shutdownAndExit:${code}`);
  await drainPools();
  releaseLock();
  process.exit(code);
}

function ensureDir(p) { fs.mkdirSync(p, { recursive: true }); }

function atomicWriteJson(targetPath, obj) {
  const dir = path.dirname(targetPath);
  ensureDir(dir);
  const tmp = path.join(dir, `.tmp.${process.pid}.${Date.now()}.${Math.random().toString(36).slice(2)}`);
  fs.writeFileSync(tmp, JSON.stringify(obj, null, 2));
  fs.renameSync(tmp, targetPath);
}

function isoDateUTC() { return new Date().toISOString().slice(0, 10); }

function sha256Prefix16(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex').slice(0, 16);
}

function classifyShowdownError(errStr) {
  if (typeof errStr !== 'string') return 'other';
  const s = errStr.toLowerCase();
  if (s.includes('trapped')) return 'trapped_switch';
  if (s.includes('locked into') || s.includes('choice') && s.includes('lock')) return 'choice_locked';
  if (s.includes('fainted') && s.includes('switch')) return 'fainted_switch';
  if (s.includes('terastallize') && (s.includes('twice') || s.includes('already'))) return 'tera_double';
  if (s.includes('disabled') || s.includes('taunt') || s.includes('encore')) return 'disable_taunt_encore';
  return 'other';
}

function spawnShowdown(scenarioJsonStr, timeoutMs) {
  return spawnSync('node', [SHOWDOWN_RUNNER], {
    input: scenarioJsonStr,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
}

function spawnEngine(scenarioJsonStr, timeoutMs) {
  return spawnSync(ENGINE_BIN, [], {
    input: scenarioJsonStr,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
}

// Pools throw a spawn-shape object on failure, so returning it keeps the caller's branches.
async function callEngine(scenarioObj, scenarioJsonStr, timeoutMs, pool) {
  const p = pool || ENGINE_POOL;
  if (p) {
    try {
      return await p.runScenario(scenarioObj, timeoutMs);
    } catch (e) {
      return e;
    }
  }
  return spawnEngine(scenarioJsonStr, timeoutMs);
}

async function callShowdown(scenarioObj, scenarioJsonStr, timeoutMs, pool) {
  const p = pool || SHOWDOWN_POOL;
  if (p) {
    try {
      return await p.runScenario(scenarioObj, timeoutMs);
    } catch (e) {
      return e;
    }
  }
  return spawnShowdown(scenarioJsonStr, timeoutMs);
}

const KNOWN_CAT_SET = new Set(SIGNATURE_CATEGORIES);
const seenUnknownCategories = new Set();

function noteUnknownCategory(cat) {
  if (seenUnknownCategories.has(cat)) return;
  seenUnknownCategories.add(cat);
  console.warn(`fuzz.js: unknown oracle category emitted: ${cat} — fail-closed (treated as required)`);
  try {
    fs.appendFileSync(
      ORACLE_SCOPE_PATH,
      `\n<!-- runtime: unknown oracle category seen: ${cat} (added ${new Date().toISOString()}) — review and promote if legitimate, else add to allowlist -->\n`,
    );
  } catch (_) {}
}

function pushCategory(set, cat) {
  if (Array.isArray(cat)) { for (const c of cat) pushCategory(set, c); return; }
  if (!KNOWN_CAT_SET.has(cat)) noteUnknownCategory(cat);
  set.add(cat);
}

function categoriesCheckedFromTurn(turn) {
  // For an all_rolls turn `state_after` is a 16-element array; enumerate only the first.
  const set = new Set();
  const stateAfter = Array.isArray(turn.state_after) ? turn.state_after[0] : turn.state_after;
  if (!stateAfter || typeof stateAfter !== 'object') return set;

  const turnIdx = (typeof turn.turn === 'number') ? turn.turn : 0;
  const rootPrefix = `turns[${turnIdx}].state_after`;

  for (const p of enumerateFieldPaths(rootPrefix, stateAfter.field)) {
    pushCategory(set, path_to_category(p));
  }

  for (const sideName of ['p1', 'p2']) {
    const sideSnap = stateAfter[sideName];
    if (!sideSnap) continue;
    const sidePrefix = `${rootPrefix}.${sideName}`;

    for (const p of enumerateTeamPaths(sidePrefix, sideSnap)) {
      pushCategory(set, path_to_category(p));
    }
    for (const p of enumerateActivePaths(`${sidePrefix}.active`, sideSnap.active)) {
      pushCategory(set, path_to_category(p));
    }
    for (const p of enumerateSideConditionPaths(`${sidePrefix}.side_conditions`, sideSnap.side_conditions)) {
      pushCategory(set, path_to_category(p));
    }
  }
  return set;
}

function categoriesCheckedFromEngineOutput(engOut) {
  const union = new Set();
  const turns = engOut && engOut.turns;
  if (!Array.isArray(turns)) return union;
  for (const t of turns) {
    const s = categoriesCheckedFromTurn(t);
    for (const c of s) union.add(c);
  }
  return union;
}

function healthCheckBuildSlot(engOut) {
  const issues = [];
  const turns = engOut && engOut.turns;
  if (!Array.isArray(turns) || turns.length === 0) {
    return { hardFail: false, warnings: [] };
  }
  const t0 = turns[0];
  const stateAfter = Array.isArray(t0.state_after) ? t0.state_after[0] : t0.state_after;
  if (!stateAfter) return { hardFail: false, warnings: [] };

  const warnings = [];
  let hardFail = false;
  for (const sideName of ['p1', 'p2']) {
    const side = stateAfter[sideName];
    if (!side || !Array.isArray(side.team)) continue;
    for (let i = 0; i < side.team.length; i++) {
      const mon = side.team[i];
      if (!mon) continue;
      // Not every snapshot carries build_slot; absence is tolerated, not a failure.
      if (!('build_slot' in mon)) continue;
      const bs = mon.build_slot;
      if (bs === null) {
        warnings.push({ side: sideName, slot: i, reason: 'build_slot_null' });
        continue;
      }
      if (!Number.isFinite(bs) || bs < 0 || bs > 5 || !Number.isInteger(bs)) {
        hardFail = true;
        issues.push({ side: sideName, slot: i, value: bs });
      }
    }
  }
  return { hardFail, warnings, issues };
}

function writeInbox(bucketRoot, category, scenarioBytes, scenarioObj, meta) {
  const date = isoDateUTC();
  const hash = sha256Prefix16(scenarioBytes);
  const dir = path.join(bucketRoot, date, category, hash);
  ensureDir(dir);
  const scenarioFinal = path.join(dir, 'scenario.json');
  // Random token: two workers can reach the same hash dir within one ms and collide on the tmp path.
  const scenarioTmp = path.join(dir, `.scenario.tmp.${process.pid}.${Date.now()}.${Math.random().toString(36).slice(2)}`);
  fs.writeFileSync(scenarioTmp, scenarioBytes);
  fs.renameSync(scenarioTmp, scenarioFinal);
  atomicWriteJson(path.join(dir, 'meta.json'), meta);
  return dir;
}

function makeMeta(fields) {
  // Every meta.json field is listed here so no finding ever writes a sparse file.
  const def = {
    seed: null,
    scenario_hash: null,
    found_at: new Date().toISOString(),
    divergent_turn: null,
    signature: [],
    bucket: null,
    minimize_verified: false,
    first_divergent_turn_preserved: false,
    suppressed: 'none',
    bug_ids: [],
    classification: null,
    showdown_rejection_reason: null,
    engine_exit: null,
  };
  return Object.assign(def, fields);
}

function appendStats(line) {
  // Once halted, drop the harness_error flood from in-flight pool rejections during drain.
  if (halt.isHalted() && line && line.result === CLASSIFICATIONS.HARNESS_ERROR) return;
  // appendFileSync of one line is atomic on the event loop; W workers must not interleave bytes.
  fs.appendFileSync(STATS_PATH, JSON.stringify(line) + '\n');
}

const TAIL_BYTES_PER_LINE = 4096;   // ≥ observed ~3.3 KB mean coverage row
const TAIL_MIN_BYTES = 1 << 20;     // 1 MiB floor for small windows

function readTrailingStats(n) {
  let fd;
  try { fd = fs.openSync(STATS_PATH, 'r'); }
  catch (_) { return []; }
  try {
    const size = fs.fstatSync(fd).size;
    if (size === 0) return [];
    let k = Math.max(TAIL_MIN_BYTES, n * TAIL_BYTES_PER_LINE);
    while (true) {
      const offset = Math.max(0, size - k);
      const readLen = size - offset;
      const buf = Buffer.allocUnsafe(readLen);
      let got = 0;
      while (got < readLen) {
        const r = fs.readSync(fd, buf, got, readLen - got, offset + got);
        if (r <= 0) break;
        got += r;
      }
      let parts = buf.toString('utf8', 0, got).split('\n');
      // A mid-record leading fragment can still parse as JSON, so drop it unconditionally.
      if (offset > 0) parts = parts.slice(1); // drop leading fragment
      const lines = parts.filter((l) => l.length > 0);
      // Key the doubling on complete lines, not parsed rows, so one bad row cannot grow the read.
      if (lines.length >= n || offset === 0) {
        const trail = lines.slice(-n);
        const parsed = [];
        for (let i = 0; i < trail.length; i++) {
          try { parsed.push(JSON.parse(trail[i])); }
          catch (_) {
            if (i === trail.length - 1) continue; // tolerate truncated final
            // Skip mid-window unparseable line silently rather than crash the gate.
          }
        }
        return parsed;
      }
      k *= 2;
    }
  } finally {
    fs.closeSync(fd);
  }
}

async function runOracleGate(windowSize, windowMultiplier, totalSeen) {
  const need = windowSize * windowMultiplier;
  if (totalSeen < need) return; // bootstrap silence
  const trail = readTrailingStats(need);
  if (trail.length === 0) return;
  const requiredEnforced = REQUIRED_CATEGORIES.filter((c) => !ORACLE_SCOPE_ALLOWLIST.has(c));
  for (const cat of requiredEnforced) {
    let hits = 0;
    for (const row of trail) {
      const cov = Array.isArray(row.oracle_coverage) ? row.oracle_coverage : [];
      if (cov.indexOf(cat) !== -1) { hits++; break; } // any non-zero suffices
    }
    if (hits === 0) {
      const startSeed = trail[0] && trail[0].seed;
      const endSeed = trail[trail.length - 1] && trail[trail.length - 1].seed;
      const report = `# HARNESS ORACLE COVERAGE GAP\n\n`
        + `Required category went silent across the trailing ${need} scenarios.\n\n`
        + `- category: \`${cat}\`\n`
        + `- window scenarios: ${trail.length}\n`
        + `- start seed: ${startSeed}\n`
        + `- end seed: ${endSeed}\n`
        + `- timestamp: ${new Date().toISOString()}\n`;
      fs.writeFileSync(HARNESS_ISSUE_ORACLE, report);
      console.error(`fuzz.js: oracle-coverage gate FAILED on category ${cat}`);
      await shutdownAndExit(1);
      return;
    }
  }
}

async function runEngineErrorFloor(totalSeen, errCount) {
  if (totalSeen < 10000) return;
  const rate = errCount / totalSeen;
  if (rate > 0.001) {
    console.error(`fuzz.js: engine_error+engine_crash rate ${rate.toFixed(5)} > 0.1% over ${totalSeen} scenarios — halting`);
    await shutdownAndExit(1);
  }
}

async function runScenario(seed, scenarioObj, config, seedMode, rngState, pools) {
  const timeout = config.scenario_timeout_ms;
  const wallStart = Date.now();
  const enginePool = (pools && pools.engine) || ENGINE_POOL;
  const showdownPool = (pools && pools.showdown) || SHOWDOWN_POOL;

  // Rebuilt after each turn extension so classification runs against the final scenario.
  let scenarioBytes = Buffer.from(JSON.stringify(scenarioObj), 'utf8');
  let scenarioStr = scenarioBytes.toString('utf8');

  const stats = {
    seed,
    turns: Array.isArray(scenarioObj.turns) ? scenarioObj.turns.length : 0,
    result: null,
    duration_ms: null,
    oracle_coverage: [],
    diverged_pre_minimize: false,
    original_divergent_turn: null,
  };

  const elapsed = () => Date.now() - wallStart;
  const outOfTime = () => elapsed() >= timeout;
  // Floor at 1 ms so the pool never sees a non-positive timeout after a wallclock check slips.
  const callTimeout = () => Math.max(1, Math.min(timeout - elapsed(), timeout));

  const routeHarnessError = (subcat, extras) => {
    const meta = makeMeta(Object.assign({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification: CLASSIFICATIONS.HARNESS_ERROR,
    }, extras || {}));
    writeInbox(INBOX_REJECTED, subcat, scenarioBytes, scenarioObj, meta);
    stats.result = CLASSIFICATIONS.HARNESS_ERROR;
  };

  const routeEngineFailure = (classification, subcat, eng) => {
    const meta = makeMeta({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification,
      engine_exit: { status: eng.status, signal: eng.signal },
      showdown_rejection_reason: (eng.stderr || '').slice(0, 4096) || null,
    });
    writeInbox(INBOX_REJECTED, subcat, scenarioBytes, scenarioObj, meta);
    stats.result = classification;
  };

  if (rngState) {
    while (true) {
      if (outOfTime()) {
        routeHarnessError('harness_error', { showdown_rejection_reason: 'scenario_timeout' });
        stats.duration_ms = seedMode ? null : elapsed();
        return stats;
      }
      let eng;
      try { eng = await timed('engine_probe', () => callEngine(scenarioObj, scenarioStr, callTimeout(), enginePool)); }
      catch (e) {
        routeHarnessError('harness_error', {
          showdown_rejection_reason: `engine_spawn_error: ${e.message}`,
        });
        stats.duration_ms = seedMode ? null : elapsed();
        return stats;
      }
      // A mid-scenario respawn changes the state distribution between probes, so bail.
      if (eng.respawned) {
        routeHarnessError('harness_error', {
          showdown_rejection_reason: `engine_pool_respawn_probe: ${eng.reason || 'unknown'}`,
        });
        stats.duration_ms = seedMode ? null : elapsed();
        return stats;
      }
      if (eng.signal !== null) {
        routeEngineFailure(CLASSIFICATIONS.ENGINE_CRASH, 'engine_crash', eng);
        stats.duration_ms = seedMode ? null : elapsed();
        return stats;
      }
      if (eng.status !== 0) {
        routeEngineFailure(CLASSIFICATIONS.ENGINE_ERROR, 'engine_error', eng);
        stats.duration_ms = seedMode ? null : elapsed();
        return stats;
      }
      let engProbeOut;
      try { engProbeOut = JSON.parse(eng.stdout || ''); }
      catch (_) {
        const meta = makeMeta({
          seed,
          scenario_hash: sha256Prefix16(scenarioBytes),
          bucket: 'rejected',
          classification: CLASSIFICATIONS.ENGINE_OUTPUT_MALFORMED,
          engine_exit: { status: eng.status, signal: eng.signal },
          showdown_rejection_reason: 'engine_unparseable',
        });
        meta.parse_error = true;
        writeInbox(INBOX_REJECTED, 'engine_error', scenarioBytes, scenarioObj, meta);
        stats.result = CLASSIFICATIONS.ENGINE_OUTPUT_MALFORMED;
        stats.duration_ms = seedMode ? null : elapsed();
        return stats;
      }

      const engTurns = Array.isArray(engProbeOut.turns) ? engProbeOut.turns : [];
      const last = engTurns[engTurns.length - 1];
      const la = (last && last.legal_actions_after) || { p1: [], p2: [] };
      const p1Bytes = Array.isArray(la.p1) ? la.p1 : [];
      const p2Bytes = Array.isArray(la.p2) ? la.p2 : [];

      if (p1Bytes.length === 0 && p2Bytes.length === 0) break;

      // Showdown collapses faint replacement into the next turn, so appending past a switch_* phase is rejected.
      const sa = last && (Array.isArray(last.state_after) ? last.state_after[0] : last.state_after);
      if (sa && sa.phase && sa.phase !== 'actions') break;

      const appended = generate.extendScenario(scenarioObj, rngState, p1Bytes, p2Bytes);
      if (!appended) break; // hit max_turns_per_scenario

      scenarioBytes = Buffer.from(JSON.stringify(scenarioObj), 'utf8');
      scenarioStr = scenarioBytes.toString('utf8');
    }
    stats.turns = scenarioObj.turns.length;
  }

  if (outOfTime()) {
    routeHarnessError('harness_error', { showdown_rejection_reason: 'scenario_timeout' });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  let sd;
  try { sd = await timed('showdown', () => callShowdown(scenarioObj, scenarioStr, callTimeout(), showdownPool)); }
  catch (e) {
    routeHarnessError('harness_error', { showdown_rejection_reason: `spawn_error: ${e.message}` });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  if (sd.respawned) {
    routeHarnessError('harness_error', {
      showdown_rejection_reason: `showdown_pool_respawn: ${sd.reason || 'unknown'}`,
    });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  if (sd.signal !== null) {
    routeHarnessError('harness_error', { showdown_rejection_reason: `signal:${sd.signal}` });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  let sdOut;
  try { sdOut = JSON.parse(sd.stdout || ''); }
  catch (_) {
    routeHarnessError('harness_error', { showdown_rejection_reason: 'showdown_unparseable' });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  if (sdOut.success === false || sdOut.error === 'Not all choices done') {
    const subcat = classifyShowdownError(sdOut.error || '');
    const meta = makeMeta({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification: CLASSIFICATIONS.SHOWDOWN_REJECTED,
      showdown_rejection_reason: sdOut.error || null,
    });
    writeInbox(INBOX_REJECTED, subcat, scenarioBytes, scenarioObj, meta);
    stats.result = CLASSIFICATIONS.SHOWDOWN_REJECTED;
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }

  if (outOfTime()) {
    routeHarnessError('harness_error', { showdown_rejection_reason: 'scenario_timeout' });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  let eng;
  try { eng = await timed('engine_final', () => callEngine(scenarioObj, scenarioStr, callTimeout(), enginePool)); }
  catch (e) {
    routeHarnessError('harness_error', { showdown_rejection_reason: `engine_spawn_error: ${e.message}` });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  if (eng.respawned) {
    routeHarnessError('harness_error', {
      showdown_rejection_reason: `engine_pool_respawn: ${eng.reason || 'unknown'}`,
    });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }

  if (eng.signal !== null) {
    const meta = makeMeta({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification: CLASSIFICATIONS.ENGINE_CRASH,
      engine_exit: { status: eng.status, signal: eng.signal },
      showdown_rejection_reason: (eng.stderr || '').slice(0, 4096) || null,
    });
    writeInbox(INBOX_REJECTED, 'engine_crash', scenarioBytes, scenarioObj, meta);
    stats.result = CLASSIFICATIONS.ENGINE_CRASH;
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  if (eng.status !== 0) {
    const meta = makeMeta({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification: CLASSIFICATIONS.ENGINE_ERROR,
      engine_exit: { status: eng.status, signal: eng.signal },
      showdown_rejection_reason: (eng.stderr || '').slice(0, 4096) || null,
    });
    writeInbox(INBOX_REJECTED, 'engine_error', scenarioBytes, scenarioObj, meta);
    stats.result = CLASSIFICATIONS.ENGINE_ERROR;
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  let engOut;
  try { engOut = JSON.parse(eng.stdout || ''); }
  catch (_) {
    const meta = makeMeta({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification: CLASSIFICATIONS.ENGINE_OUTPUT_MALFORMED,
      engine_exit: { status: eng.status, signal: eng.signal },
      showdown_rejection_reason: 'engine_unparseable',
    });
    meta.parse_error = true;
    writeInbox(INBOX_REJECTED, 'engine_error', scenarioBytes, scenarioObj, meta);
    stats.result = CLASSIFICATIONS.ENGINE_OUTPUT_MALFORMED;
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }

  const health = healthCheckBuildSlot(engOut);
  if (health.hardFail) {
    const meta = makeMeta({
      seed,
      scenario_hash: sha256Prefix16(scenarioBytes),
      bucket: 'rejected',
      classification: CLASSIFICATIONS.HARNESS_ERROR,
      showdown_rejection_reason: `build_slot_out_of_range: ${JSON.stringify(health.issues || [])}`,
    });
    writeInbox(INBOX_REJECTED, 'harness_health', scenarioBytes, scenarioObj, meta);
    stats.result = CLASSIFICATIONS.HARNESS_ERROR;
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  if (health.warnings && health.warnings.length > 0) {
    for (const w of health.warnings) {
      console.warn(`fuzz.js: build_slot_null seed=${seed} side=${w.side} slot=${w.slot}`);
    }
  }

  const checkedSet = categoriesCheckedFromEngineOutput(engOut);
  stats.oracle_coverage = Array.from(checkedSet).sort();

  if (outOfTime()) {
    routeHarnessError('harness_error', { showdown_rejection_reason: 'scenario_timeout' });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }
  let cmpOut;
  try { cmpOut = await timed('compare', () => compareResults(sdOut, engOut, FORME_MAP)); }
  catch (e) {
    routeHarnessError('harness_error', { showdown_rejection_reason: 'comparator_stdout_unparseable' });
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }

  const diffs = Array.isArray(cmpOut.diffs) ? cmpOut.diffs : [];

  stats.diverged_pre_minimize = diffs.length > 0;

  if (cmpOut.verdict === 'PASS') {
    stats.result = CLASSIFICATIONS.PASS;
    stats.duration_ms = seedMode ? null : elapsed();
    return stats;
  }

  let originalDivergentTurn = null;
  for (const d of diffs) {
    if (!d || typeof d.path !== 'string') continue;
    const m = d.path.match(/^turns\[(\d+)\]\./);
    if (!m) continue;
    const t = parseInt(m[1], 10);
    if (originalDivergentTurn === null || t < originalDivergentTurn) {
      originalDivergentTurn = t;
    }
  }
  // A turns-length-only FAIL leaves this null; diverged_pre_minimize is the divergence authority.
  stats.original_divergent_turn = originalDivergentTurn;

  const originalSignature = minimize.buildSignature(
    Array.isArray(engOut && engOut.turns) ? engOut.turns : [],
    diffs,
    scenarioObj,
  );

  const runOracle = async (cand) => {
    const candStr = JSON.stringify(cand);
    const sd2 = await callShowdown(cand, candStr, callTimeout(), showdownPool);
    if (sd2.respawned) return { success: false };
    if (sd2.signal !== null) return { success: false };
    let sd2Out;
    try { sd2Out = JSON.parse(sd2.stdout || ''); } catch (_) { return { success: false }; }
    if (sd2Out.success === false || sd2Out.error === 'Not all choices done') {
      return { success: false };
    }
    const eng2 = await callEngine(cand, candStr, callTimeout(), enginePool);
    if (eng2.respawned) return { success: false };
    if (eng2.signal !== null || eng2.status !== 0) return { success: false };
    let eng2Out;
    try { eng2Out = JSON.parse(eng2.stdout || ''); } catch (_) { return { success: false }; }
    // Must fall through to the empty-diffs success path; an early return would change the signature.
    let cmp2Diffs = [];
    try {
      const cmp2 = compareResults(sd2Out, eng2Out, FORME_MAP);
      cmp2Diffs = Array.isArray(cmp2.diffs) ? cmp2.diffs : [];
    } catch (_) {
      cmp2Diffs = [];
    }
    let dt = null;
    for (const d of cmp2Diffs) {
      if (!d || typeof d.path !== 'string') continue;
      const m = d.path.match(/^turns\[(\d+)\]\./);
      if (!m) continue;
      const tt = parseInt(m[1], 10);
      if (dt === null || tt < dt) dt = tt;
    }
    return {
      success: true,
      signature: minimize.buildSignature(
        Array.isArray(eng2Out.turns) ? eng2Out.turns : [],
        cmp2Diffs,
        cand,
      ),
      diffs: cmp2Diffs,
      divergent_turn: dt,
      turn_results: Array.isArray(eng2Out.turns) ? eng2Out.turns : [],
    };
  };

  let minResult;
  try {
    minResult = await timed('minimize', () => minimize.minimize(
      scenarioObj,
      { diffs, divergent_turn: originalDivergentTurn },
      originalSignature,
      config,
      runOracle,
    ));
  } catch (e) {
    minResult = {
      scenario: scenarioObj,
      signature: Array.from(originalSignature || []),
      minimize_verified: false,
      first_divergent_turn_preserved: false,
    };
  }

  const reInitialTeam = {
    p1: ((minResult.scenario.teams && minResult.scenario.teams.p1) || []).map((m) => ({
      species_id: m.species_id, ability_id: m.ability_id, item_id: m.item_id,
      move_ids: Array.isArray(m.moves) ? m.moves.slice() : [],
    })),
    p2: ((minResult.scenario.teams && minResult.scenario.teams.p2) || []).map((m) => ({
      species_id: m.species_id, ability_id: m.ability_id, item_id: m.item_id,
      move_ids: Array.isArray(m.moves) ? m.moves.slice() : [],
    })),
  };

  // Best-effort: entity sets come from the un-minimized run, which the suppressor tolerates as sparse.
  const reEntitySets = generate.extractEntitySets(
    Array.isArray(engOut && engOut.turns) ? engOut.turns : [],
  );
  // extractEntitySets keys from turn 1, so turn 0 must be seeded from initial_team.
  reEntitySets[0] = {
    p1: reInitialTeam.p1[0] || null,
    p2: reInitialTeam.p2[0] || null,
  };

  let supp;
  try {
    supp = suppress.classify({
      signature: minResult.signature,
      entity_sets_by_turn: reEntitySets,
      initial_team: reInitialTeam,
      divergent_turn: originalDivergentTurn,
      knownBugs: KNOWN_BUGS_CACHE,
    });
  } catch (e) { supp = { level: CLASSIFICATIONS.NONE, bug_ids: [] }; }

  let classification;
  let bucketRoot;
  let bucketName;
  let suppressedTag = CLASSIFICATIONS.NONE;
  if (supp.level === CLASSIFICATIONS.HIGH) {
    classification = CLASSIFICATIONS.SUPPRESSED_HIGH;
    bucketRoot = INBOX_SUPPRESSED;
    bucketName = 'suppressed';
    suppressedTag = CLASSIFICATIONS.HIGH;
  } else if (supp.level === CLASSIFICATIONS.LOW) {
    classification = CLASSIFICATIONS.NEW_BUG_LOW_CONF;
    bucketRoot = INBOX_BUGS;
    bucketName = 'bugs';
    suppressedTag = CLASSIFICATIONS.LOW;
  } else {
    classification = CLASSIFICATIONS.NEW_BUG;
    bucketRoot = INBOX_BUGS;
    bucketName = 'bugs';
    suppressedTag = CLASSIFICATIONS.NONE;
  }

  // suppressed_splash exists so the splash audit still sees findings suppression would absorb.
  if (classification === CLASSIFICATIONS.NEW_BUG && signatureMentionsSplash(minResult.signature)) {
    bucketRoot = INBOX_SPLASH;
    bucketName = 'splash';
  } else if (classification === CLASSIFICATIONS.SUPPRESSED_HIGH
             && signatureMentionsSplash(minResult.signature)) {
    bucketRoot = INBOX_SUPPRESSED_SPLASH;
    bucketName = 'suppressed_splash';
  }

  let primaryCat = 'unknown';
  const sigArr = Array.isArray(minResult.signature) ? minResult.signature : Array.from(minResult.signature || []);
  for (const sig of sigArr) {
    const c = (typeof sig === 'string') ? sig : (sig && sig.category);
    if (typeof c === 'string') { primaryCat = c; break; }
  }

  const minScenarioBytes = Buffer.from(JSON.stringify(minResult.scenario), 'utf8');
  const sigStrings = sigArr.map((s) => {
    if (typeof s === 'string') return s;
    if (s && typeof s.category === 'string') return s.category;
    return null;
  }).filter(Boolean);

  const meta = makeMeta({
    seed,
    scenario_hash: sha256Prefix16(minScenarioBytes),
    divergent_turn: (typeof minResult.divergent_turn === 'number') ? minResult.divergent_turn : null,
    signature: sigStrings,
    bucket: bucketName,
    minimize_verified: !!minResult.minimize_verified,
    first_divergent_turn_preserved: !!minResult.first_divergent_turn_preserved,
    suppressed: suppressedTag,
    bug_ids: Array.isArray(supp.bug_ids) ? supp.bug_ids : [],
    classification,
    engine_exit: { status: eng.status, signal: eng.signal },
  });

  writeInbox(bucketRoot, primaryCat, minScenarioBytes, minResult.scenario, meta);

  stats.result = classification;
  stats.duration_ms = seedMode ? null : elapsed();
  return stats;
}

function emitStartupSummary() {
  let kbCounts = { abilities: 0, items: 0, moves: 0, species: 0 };
  let kbPresent = 'no';
  if (fs.existsSync(KNOWN_BUGS_PATH)) {
    kbPresent = 'yes';
    try {
      const kb = JSON.parse(fs.readFileSync(KNOWN_BUGS_PATH, 'utf8'));
      for (const axis of ['abilities', 'items', 'moves', 'species']) {
        const v = kb[axis];
        kbCounts[axis] = (v && typeof v === 'object') ? Object.keys(v).length : 0;
      }
    } catch (_) { /* tolerate */ }
  }
  const formePresent = fs.existsSync(FORME_MAP_PATH) ? 'yes' : 'no';
  console.log(
    `fuzz.js startup: known_divergences.json present: ${kbPresent} `
    + `(abilities: ${kbCounts.abilities} entries, items: ${kbCounts.items}, `
    + `moves: ${kbCounts.moves}, species: ${kbCounts.species}); `
    + `engine_forme_to_showdown.json present: ${formePresent}`,
  );
}

async function runUnitTestInvalidChoice() {
  // Byte 5 switches to a nonexistent slot; a not-a-PASS tripwire, so the rejection kind is not pinned.
  const fixture = {
    name: 'unit_test_invalid_choice',
    mode: 'execute_turns',
    teams: {
      p1: [{ species_id: 25, ability_id: 9, item_id: 0, moves: [33, 0, 0, 0] }],
      p2: [{ species_id: 25, ability_id: 9, item_id: 0, moves: [33, 0, 0, 0] }],
    },
    turns: [
      { p1_action: 5, p2_action: 0, rng_mode: 'force_all' },
    ],
  };

  const config = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'));
  const stats = await runScenario(0, fixture, config, true);

  const ok = stats.result === CLASSIFICATIONS.SHOWDOWN_REJECTED
    || stats.result === CLASSIFICATIONS.HARNESS_ERROR
    || stats.result === CLASSIFICATIONS.ENGINE_ERROR;
  if (!ok) {
    console.error(`fuzz.js: unit-test invalid-choice FAILED — result=${stats.result}`);
    await shutdownAndExit(1);
    return;
  }
  console.log(`fuzz.js: unit-test invalid-choice PASSED — result=${stats.result}`);
  await shutdownAndExit(0);
}

async function runFixtureMode(fixturePath) {
  const config = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'));
  let scenarioObj;
  try { scenarioObj = JSON.parse(fs.readFileSync(fixturePath, 'utf8')); }
  catch (e) {
    console.error(`fuzz.js: failed to load fixture ${fixturePath}: ${e.message}`);
    await shutdownAndExit(2);
    return;
  }
  const stats = await runScenario(0, scenarioObj, config, true);
  appendStats(stats);
}

async function main() {
  acquireLock();
  KNOWN_BUGS_CACHE = suppress.loadKnownBugs(KNOWN_BUGS_PATH);
  FORME_MAP = loadFormeMap();
  emitStartupSummary();

  ensureDir(INBOX_BUGS); ensureDir(INBOX_SUPPRESSED); ensureDir(INBOX_SUPPRESSED_SPLASH); ensureDir(INBOX_REJECTED); ensureDir(INBOX_SPLASH);

  const config = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'));

  const useServerMode = process.env.FUZZ_USE_SERVER_MODE === '1'
    ? true
    : (process.env.FUZZ_USE_SERVER_MODE === '0' ? false : !!config.use_server_mode);

  const envW = parseInt(process.env.FUZZ_CONCURRENCY || '', 10);
  // The computed default leaves a core free: Showdown is CPU-bound JS.
  const W = (Number.isFinite(envW) && envW > 0) ? envW
    : ((Number.isFinite(config.concurrency) && config.concurrency > 0) ? config.concurrency
    : Math.max(1, Math.min(os.cpus().length - 1, 6)));
  if (useServerMode) {
    const { SubprocessPool } = require('./lib/subprocess_pool.js');
    const makePoolPair = () => ({
      engine: new SubprocessPool({
        command: ENGINE_BIN, args: ['--server-mode'], label: 'engine', kind: 'engine',
        recycleAfter: config.recycleAfter || 1000, rssCapMb: config.rss_cap_engine_mb || 1024,
      }),
      showdown: new SubprocessPool({
        command: 'node', args: [SHOWDOWN_RUNNER, '--server-mode'], label: 'showdown', kind: 'showdown',
        recycleAfter: config.recycleAfter || 1000, rssCapMb: config.rss_cap_showdown_mb || 2048,
      }),
    });
    const poolCount = (ARGV.unitTest || ARGV.fixture) ? 1 : W;
    for (let w = 0; w < poolCount; w++) POOL_PAIRS.push(makePoolPair());
    ENGINE_POOL = POOL_PAIRS[0].engine;
    SHOWDOWN_POOL = POOL_PAIRS[0].showdown;
  }

  // The synchronous 'exit' handler cannot await pool shutdown, so drain here instead.
  process.on('uncaughtException', async (err) => {
    console.error(`fuzz.js: uncaughtException: ${err && (err.stack || err.message) || err}`);
    await shutdownAndExit(1);
  });
  process.on('unhandledRejection', async (err) => {
    console.error(`fuzz.js: unhandledRejection: ${err && (err.stack || err.message) || err}`);
    await shutdownAndExit(1);
  });

  if (ARGV.unitTest === 'invalid-choice') {
    await runUnitTestInvalidChoice();
    return;
  }
  if (ARGV.fixture) {
    await runFixtureMode(ARGV.fixture);
    await drainPools();
    return;
  }

  const max = ARGV.maxScenarios ?? Infinity;
  const seedMode = ARGV.seed !== null;
  const effectiveSeed = (ARGV.seed ?? config.seed ?? 0) >>> 0;

  // No await between read and write of these counters, so no two workers see the same value.
  let totalSeen = 0;
  let engineErrCount = 0;
  let nextIndex = 0;
  const mainLoopStart = Date.now();

  const claimIndex = () => (nextIndex < max ? nextIndex++ : -1);

  const poolFatallyDead = (pool) => {
    if (!pool) return false;                          // legacy mode, no pool
    if (pool.shuttingDown) return true;
    if (!pool.child) return true;                     // _spawn skipped (shuttingDown race)
    if (!pool.child.stdin || !pool.child.stdin.writable) return true;
    return false;
  };

  const pipelines = POOL_PAIRS.length ? POOL_PAIRS : [{ engine: null, showdown: null }];

  const worker = async (pipe) => {
    while (true) {
      if (halt.isHalted()) break;                     // boundary halt check
      if (poolFatallyDead(pipe.engine) || poolFatallyDead(pipe.showdown)) {
        console.error('fuzz.js: pool fatally dead — halting workers to ' +
                      'avoid synthetic-harness_error flood');
        halt.halt('pool_fatally_dead');
        break;
      }
      const i = claimIndex();
      if (i < 0) break;                               // max reached
      const seed = effectiveSeed + i;

      let genResult;
      try { genResult = await timed('generate', () => generate.generateScenario(seed, config, pipe.engine)); }
      catch (e) {
        console.error(`fuzz.js: generator failed at seed ${seed}: ${e.message}`);
        // Emit a stats row so the coverage gate still counts this scenario.
        appendStats({ seed, turns: 0, result: CLASSIFICATIONS.HARNESS_ERROR, duration_ms: seedMode ? null : 0, oracle_coverage: [], diverged_pre_minimize: false, original_divergent_turn: null });
        totalSeen++;
        continue;
      }

      const stats = await runScenario(seed, genResult.scenario, config, seedMode, genResult.rngState, pipe);
      appendStats(stats);

      // Increment then test synchronously so one worker owns each boundary; membership drifts up to W-1 rows.
      totalSeen++;
      const seenSnapshot = totalSeen;
      const atBoundary = (seenSnapshot % config.window_size === 0);
      if (stats.result === CLASSIFICATIONS.ENGINE_ERROR
        || stats.result === CLASSIFICATIONS.ENGINE_CRASH
        || stats.result === CLASSIFICATIONS.ENGINE_OUTPUT_MALFORMED) {
        engineErrCount++;
      }

      if (pipe.engine && pipe.engine.recyclePending) await pipe.engine.recycleNow();
      if (pipe.showdown && pipe.showdown.recyclePending) await pipe.showdown.recycleNow();

      if (atBoundary) {
        await runOracleGate(config.window_size, config.window_multiplier, seenSnapshot);
        await runEngineErrorFloor(seenSnapshot, engineErrCount);
      }
    }
  };

  await Promise.all(pipelines.map((p) => worker(p)));

  emitPhaseTimingSummary(totalSeen, Date.now() - mainLoopStart);
  await drainPools();
}

module.exports = { readTrailingStats };

if (require.main === module) {
  main().catch(async (e) => {
    console.error(`fuzz.js: top-level failure: ${e && (e.stack || e.message) || e}`);
    await shutdownAndExit(1);
  });
}
