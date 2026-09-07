#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const {spawnSync} = require('child_process');

const FUZZER_DIR = path.join(__dirname, '..');
const HARNESS_DIR = path.join(FUZZER_DIR, '..', 'harness');
const ENGINE_BIN = path.join(HARNESS_DIR, 'run_scenario', 'target', 'debug', 'run_scenario');
const SHOWDOWN_RUNNER = path.join(HARNESS_DIR, 'showdown_runner.js');
const COMPARE_RESULTS = path.join(HARNESS_DIR, 'compare_results.js');
const REPORT_PATH = path.join(__dirname, 'parity_report.json');

const {SubprocessPool} = require(path.join(FUZZER_DIR, 'lib', 'subprocess_pool.js'));
const generate = require(path.join(FUZZER_DIR, 'generate.js'));
const minimize = require(path.join(FUZZER_DIR, 'minimize.js'));

const ARGV = (() => {
  const a = {smoke: false, pass: 'all', seeds: null, startSeed: 0,
             pass4Fixtures: 20, scenarioTimeoutMs: 30000,
             minimizeBudgetSeconds: 15};
  for (let i = 2; i < process.argv.length; i++) {
    const v = process.argv[i];
    if (v === '--smoke') a.smoke = true;
    else if (v === '--pass') a.pass = process.argv[++i];
    else if (v === '--seeds') a.seeds = parseInt(process.argv[++i], 10);
    else if (v === '--start-seed') a.startSeed = parseInt(process.argv[++i], 10);
    else if (v === '--pass4-fixtures') a.pass4Fixtures = parseInt(process.argv[++i], 10);
    else if (v === '--scenario-timeout-ms') a.scenarioTimeoutMs = parseInt(process.argv[++i], 10);
    else if (v === '--minimize-budget-seconds') a.minimizeBudgetSeconds = parseInt(process.argv[++i], 10);
  }
  return a;
})();

// Mirrors compare_results.js so parity is measured under the comparator's equivalence relation.
const STATUS_MAP = {brn: 'burn', par: 'paralysis', psn: 'poison', tox: 'bad_poison',
                    slp: 'sleep', frz: 'freeze', fnt: 'none'};
function normalizeStatus(s) {
  if (!s || s === 'none' || s === '') return 'none';
  return STATUS_MAP[s] || s;
}
const WEATHER_MAP = {
  sunnyday: 'sun', sun: 'sun',
  raindance: 'rain', rain: 'rain',
  sandstorm: 'sand', sand: 'sand', sandstream: 'sand',
  snow: 'snow', snowscape: 'snow',
  desolateland: 'harsh_sun', harsh_sun: 'harsh_sun',
  primordialsea: 'heavy_rain', heavy_rain: 'heavy_rain',
  deltastream: 'strong_winds', strong_winds: 'strong_winds',
};
function normalizeWeather(w) {
  if (!w || w === '' || w === 'none') return 'none';
  return WEATHER_MAP[String(w).toLowerCase()] || w;
}
const TERRAIN_MAP = {
  electricterrain: 'electric', electric: 'electric',
  grassyterrain: 'grassy', grassy: 'grassy',
  psychicterrain: 'psychic', psychic: 'psychic',
  mistyterrain: 'misty', misty: 'misty',
};
function normalizeTerrain(t) {
  if (!t || t === '' || t === 'none') return 'none';
  return TERRAIN_MAP[String(t).toLowerCase()] || t;
}

function normalizeForParity(resp) {
  if (resp === null || typeof resp !== 'object') return resp;
  const out = JSON.parse(JSON.stringify(resp));
  // __req_id exists only in server mode, so it must never reach the hash.
  delete out.__req_id;

  // state_after is an object, or an array of 16 for an all_rolls turn.
  if (Array.isArray(out.turns)) {
    for (const turn of out.turns) {
      if (!turn || typeof turn !== 'object') continue;
      delete turn.__req_id;
      const sa = turn.state_after;
      if (Array.isArray(sa)) {
        for (const snap of sa) normalizeSnapshot(snap);
      } else if (sa && typeof sa === 'object') {
        normalizeSnapshot(sa);
      }
      if (Array.isArray(turn.state_after_all_rolls)) {
        for (const snap of turn.state_after_all_rolls) normalizeSnapshot(snap);
      }
    }
  }
  return out;
}

function normalizeSnapshot(snap) {
  if (!snap || typeof snap !== 'object') return;
  if (snap.field) {
    if ('weather' in snap.field) snap.field.weather = normalizeWeather(snap.field.weather);
    if ('terrain' in snap.field) snap.field.terrain = normalizeTerrain(snap.field.terrain);
  }
  for (const sideName of ['p1', 'p2']) {
    const side = snap[sideName];
    if (!side) continue;
    if (Array.isArray(side.team)) {
      for (const m of side.team) {
        if (m && 'status' in m) m.status = normalizeStatus(m.status);
      }
    }
    if (side.side_conditions && typeof side.side_conditions === 'object') {
      // Showdown nests layers/duration in an object; the engine stores a scalar.
      const sc = side.side_conditions;
      for (const k of Object.keys(sc)) {
        const v = sc[k];
        if (v && typeof v === 'object') {
          if ('layers' in v) sc[k] = (typeof v.layers === 'number') ? v.layers : 1;
          else if ('duration' in v) sc[k] = (typeof v.duration === 'number') ? v.duration : 0;
        } else if (v === true) sc[k] = 1;
        else if (v === false) sc[k] = 0;
      }
    }
  }
}

// canonical: recursively-sorted-keys JSON, compact.
function canonical(v) {
  if (v === null || typeof v !== 'object') return JSON.stringify(v);
  if (Array.isArray(v)) return '[' + v.map(canonical).join(',') + ']';
  const keys = Object.keys(v).sort();
  return '{' + keys.map((k) => JSON.stringify(k) + ':' + canonical(v[k])).join(',') + '}';
}

function parityHashOfStdout(stdout) {
  let parsed;
  try { parsed = JSON.parse(stdout); }
  catch (_) { return 'PARSE_FAIL:' + crypto.createHash('sha256').update(stdout).digest('hex').slice(0, 16); }
  const norm = normalizeForParity(parsed);
  const canon = canonical(norm);
  return crypto.createHash('sha256').update(canon).digest('hex');
}

function legacyEngine(scenarioObj, timeoutMs) {
  return spawnSync(ENGINE_BIN, [], {
    input: JSON.stringify(scenarioObj),
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
}
function legacyShowdown(scenarioObj, timeoutMs) {
  return spawnSync('node', [SHOWDOWN_RUNNER], {
    input: JSON.stringify(scenarioObj),
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
}
function legacyComparator(sdPath, engPath, timeoutMs) {
  return spawnSync('node', [COMPARE_RESULTS, sdPath, engPath], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
}

// Both modes execute the same final scenario, so any drift is a runtime leak, not generation drift.
async function buildScenario(seed, config, enginePool, maxTurns) {
  const cfg = {
    ...config,
    max_turns_per_scenario: maxTurns,
  };
  const r = await generate.generateScenario(seed, cfg, enginePool);
  const scenarioObj = r.scenario;
  const rngState = r.rngState;

  // Mirrors the extension loop in fuzz.js runScenario; the two must stay in step.
  while (true) {
    const probe = await enginePool.runScenario(scenarioObj, ARGV.scenarioTimeoutMs);
    if (probe.respawned || probe.signal !== null || probe.status !== 0) {
      // A partial scenario is fine: both paths then hash the same shape.
      break;
    }
    let probeOut;
    try { probeOut = JSON.parse(probe.stdout || ''); } catch (_) { break; }
    const turns = Array.isArray(probeOut.turns) ? probeOut.turns : [];
    const last = turns[turns.length - 1];
    const la = (last && last.legal_actions_after) || {p1: [], p2: []};
    const p1Bytes = Array.isArray(la.p1) ? la.p1 : [];
    const p2Bytes = Array.isArray(la.p2) ? la.p2 : [];
    if (p1Bytes.length === 0 && p2Bytes.length === 0) break;
    const sa = last && (Array.isArray(last.state_after) ? last.state_after[0] : last.state_after);
    if (sa && sa.phase && sa.phase !== 'actions') break;
    const appended = generate.extendScenario(scenarioObj, rngState, p1Bytes, p2Bytes);
    if (!appended) break;
  }
  return scenarioObj;
}

// Track ID coverage but never skip non-novel seeds; skipping blows up the scan budget.
class DexStratifier {
  constructor() {
    this.seenSpecies = new Set();
    this.seenItems = new Set();
    this.novelCount = 0;
    this.repeatCount = 0;
  }
  observe(scenarioObj) {
    if (!scenarioObj || !scenarioObj.teams) return false;
    let novel = false;
    for (const sideName of ['p1', 'p2']) {
      const side = scenarioObj.teams[sideName] || [];
      for (const m of side) {
        if (m.species_id && !this.seenSpecies.has(m.species_id)) { this.seenSpecies.add(m.species_id); novel = true; }
        if (m.item_id    && !this.seenItems.has(m.item_id))     { this.seenItems.add(m.item_id);    novel = true; }
      }
    }
    if (novel) this.novelCount++; else this.repeatCount++;
    return novel;
  }
  summary() {
    return {
      distinctSpecies: this.seenSpecies.size,
      distinctItems: this.seenItems.size,
      novelSeeds: this.novelCount,
      repeatSeeds: this.repeatCount,
    };
  }
}

async function preflightCanaryInversion(scenarioObj, serverHash) {
  // Re-hash the same scenario after one-byte mutation; MUST diverge.
  const stdout = JSON.stringify(scenarioObj);
  const mutated = stdout.replace(/"current_hp":\s*(\d+)/, (m, d) => {
    return `"current_hp": ${(parseInt(d, 10) + 1)}`;
  });
  if (mutated === stdout) {
    const obj = JSON.parse(stdout);
    obj.__canary_extra__ = 1;
    const h2 = parityHashOfStdout(JSON.stringify(obj));
    return h2 !== serverHash;
  }
  const h2 = parityHashOfStdout(mutated);
  return h2 !== serverHash;
}

async function preflightSelfValidation(scenarioObj, timeoutMs) {
  // Two legacy runs MUST hash identically.
  const r1 = legacyEngine(scenarioObj, timeoutMs);
  const r2 = legacyEngine(scenarioObj, timeoutMs);
  if (r1.status !== 0 || r2.status !== 0) {
    return {ok: false, reason: `legacy engine non-zero: r1.status=${r1.status} r2.status=${r2.status}`};
  }
  const h1 = parityHashOfStdout(r1.stdout);
  const h2 = parityHashOfStdout(r2.stdout);
  return {ok: h1 === h2, h1, h2, reason: h1 === h2 ? 'identical' : 'NON-DETERMINISTIC'};
}

async function runOnePassEngine(passLabel, seedCount, recycleAfter, startSeed,
                                 progressEvery) {
  const config = {
    max_turns_per_scenario: 4,
    rng_mode_split: {force_all: 1.0},          // disable all_rolls — keeps lines small & comparable
  };
  const scenarioTimeoutMs = ARGV.scenarioTimeoutMs;

  const buildPool = new SubprocessPool({
    command: ENGINE_BIN, args: ['--server-mode'],
    label: 'engine-build', kind: 'engine',
    recycleAfter: Infinity,
  });
  const serverEngPool = new SubprocessPool({
    command: ENGINE_BIN, args: ['--server-mode'],
    label: 'engine-server', kind: 'engine',
    recycleAfter,
  });
  const serverSdPool = new SubprocessPool({
    command: 'node', args: [SHOWDOWN_RUNNER, '--server-mode'],
    label: 'showdown-server', kind: 'showdown',
    recycleAfter,
  });

  const stratifier = new DexStratifier();
  const results = {
    pass: passLabel, recycleAfter,
    target: seedCount, evaluated: 0,
    engineDiverged: 0, showdownDiverged: 0,
    firstEngineDivergence: null, firstShowdownDivergence: null,
    errors: [],
  };

  const t0 = Date.now();
  let seed = startSeed;

  try {
    while (results.evaluated < seedCount) {
      const thisSeed = seed++;
      let scenarioObj;
      try {
        scenarioObj = await buildScenario(thisSeed, config, buildPool, 4);
      } catch (e) {
        results.errors.push({seed: thisSeed, stage: 'build', msg: e.message});
        continue;
      }

      stratifier.observe(scenarioObj);
      results.evaluated++;

      const legacySd = legacyShowdown(scenarioObj, scenarioTimeoutMs);
      const legacyEng = legacyEngine(scenarioObj, scenarioTimeoutMs);
      let serverSd, serverEng;
      try { serverSd = await serverSdPool.runScenario(scenarioObj, scenarioTimeoutMs); }
      catch (e) { serverSd = {status: 1, signal: null, stdout: '', stderr: e.message || ''}; }
      try { serverEng = await serverEngPool.runScenario(scenarioObj, scenarioTimeoutMs); }
      catch (e) { serverEng = {status: 1, signal: null, stdout: '', stderr: e.message || ''}; }

      const lSdH = parityHashOfStdout(legacySd.stdout || '');
      const sSdH = parityHashOfStdout(serverSd.stdout || '');
      const lEngH = parityHashOfStdout(legacyEng.stdout || '');
      const sEngH = parityHashOfStdout(serverEng.stdout || '');

      if (lSdH !== sSdH) {
        results.showdownDiverged++;
        if (!results.firstShowdownDivergence) {
          results.firstShowdownDivergence = {
            seed: thisSeed, evalIndex: results.evaluated - 1,
            legacyHash: lSdH, serverHash: sSdH,
            legacyStdoutLen: (legacySd.stdout || '').length,
            serverStdoutLen: (serverSd.stdout || '').length,
            legacyStderrTail: (legacySd.stderr || '').slice(-512),
            serverStderrTail: (serverSd.stderr || '').slice(-512),
          };
        }
      }
      if (lEngH !== sEngH) {
        results.engineDiverged++;
        if (!results.firstEngineDivergence) {
          results.firstEngineDivergence = {
            seed: thisSeed, evalIndex: results.evaluated - 1,
            legacyHash: lEngH, serverHash: sEngH,
            legacyStdoutLen: (legacyEng.stdout || '').length,
            serverStdoutLen: (serverEng.stdout || '').length,
            legacyStderrTail: (legacyEng.stderr || '').slice(-512),
            serverStderrTail: (serverEng.stderr || '').slice(-512),
          };
        }
      }

      if (progressEvery && (results.evaluated % progressEvery) === 0) {
        const elapsed = (Date.now() - t0) / 1000;
        const rate = results.evaluated / elapsed;
        const eta = (seedCount - results.evaluated) / Math.max(rate, 0.001);
        console.log(`  [${passLabel}] ${results.evaluated}/${seedCount} ` +
                    `(${rate.toFixed(1)}/s, eta ${(eta/60).toFixed(1)}m, ` +
                    `engDiv=${results.engineDiverged} sdDiv=${results.showdownDiverged})`);
      }

      // Drain recycle-pending at the scenario boundary, as fuzz.js does.
      if (serverEngPool.recyclePending) await serverEngPool.recycleNow();
      if (serverSdPool.recyclePending) await serverSdPool.recycleNow();
    }
  } finally {
    await Promise.allSettled([
      buildPool.shutdown(),
      serverEngPool.shutdown(),
      serverSdPool.shutdown(),
    ]);
  }

  results.elapsedSec = (Date.now() - t0) / 1000;
  results.dexCoverage = stratifier.summary();
  results.passed = results.engineDiverged === 0 && results.showdownDiverged === 0
                   && results.evaluated >= seedCount;
  return results;
}

async function discoverDivergentFixtures(targetCount, startSeed) {
  // Require turns >= 3 so the minimizer has something to shrink.
  const config = {
    max_turns_per_scenario: 6,
    rng_mode_split: {force_all: 1.0},
  };
  const buildPool = new SubprocessPool({
    command: ENGINE_BIN, args: ['--server-mode'],
    label: 'engine-build4', kind: 'engine',
    recycleAfter: Infinity,
  });
  const fixtures = [];
  const tmpDir = path.join(__dirname, 'parity_tmp');
  fs.mkdirSync(tmpDir, {recursive: true});
  let seed = startSeed;
  let scanned = 0;
  const maxScan = Math.max(2000, targetCount * 200);

  try {
    while (fixtures.length < targetCount && scanned < maxScan) {
      scanned++;
      const thisSeed = seed++;
      let scenarioObj;
      try { scenarioObj = await buildScenario(thisSeed, config, buildPool, 6); }
      catch (_) { continue; }
      if (!scenarioObj.turns || scenarioObj.turns.length < 3) continue;

      const sd = legacyShowdown(scenarioObj, ARGV.scenarioTimeoutMs);
      if (sd.status !== 0) continue;
      let sdOut; try { sdOut = JSON.parse(sd.stdout || ''); } catch (_) { continue; }
      if (sdOut.success === false || sdOut.error === 'Not all choices done') continue;

      const eng = legacyEngine(scenarioObj, ARGV.scenarioTimeoutMs);
      if (eng.status !== 0) continue;
      let engOut; try { engOut = JSON.parse(eng.stdout || ''); } catch (_) { continue; }

      const sdT = path.join(tmpDir, `discover.sd.${thisSeed}.json`);
      const engT = path.join(tmpDir, `discover.eng.${thisSeed}.json`);
      fs.writeFileSync(sdT, sd.stdout);
      fs.writeFileSync(engT, eng.stdout);
      const cmp = legacyComparator(sdT, engT, ARGV.scenarioTimeoutMs);
      try { fs.unlinkSync(sdT); fs.unlinkSync(engT); } catch (_) { /* ignore */ }
      let cmpOut; try { cmpOut = JSON.parse(cmp.stdout || ''); } catch (_) { continue; }
      if (!cmpOut || !Array.isArray(cmpOut.diffs) || cmpOut.diffs.length === 0) continue;

      // Build originalSignature so the minimizer baseline doesn't short-circuit.
      const sig = minimize.buildSignature(
        Array.isArray(engOut.turns) ? engOut.turns : [],
        cmpOut.diffs,
        scenarioObj,
      );
      if (sig.size === 0) continue;

      let originalDivergentTurn = null;
      for (const d of cmpOut.diffs) {
        if (!d || typeof d.path !== 'string') continue;
        const m = d.path.match(/^turns\[(\d+)\]\./);
        if (!m) continue;
        const t = parseInt(m[1], 10);
        if (originalDivergentTurn === null || t < originalDivergentTurn) {
          originalDivergentTurn = t;
        }
      }

      fixtures.push({
        seed: thisSeed,
        scenario: scenarioObj,
        diffs: cmpOut.diffs,
        signature: Array.from(sig).sort(),
        divergent_turn: originalDivergentTurn,
      });
      console.log(`  fixture ${fixtures.length}/${targetCount}: seed=${thisSeed} ` +
                  `turns=${scenarioObj.turns.length} diffs=${cmpOut.diffs.length} sig=${sig.size}`);
    }
  } finally {
    await buildPool.shutdown();
  }
  return {fixtures, scanned};
}

// These must return the same shape as the runOracle fuzz.js builds.
function makeLegacyOracle(timeoutMs) {
  const tmpDir = path.join(__dirname, 'parity_tmp');
  fs.mkdirSync(tmpDir, {recursive: true});
  return async (cand) => {
    const sdT = path.join(tmpDir, `oracle.${process.pid}.legacy.sd.json`);
    const engT = path.join(tmpDir, `oracle.${process.pid}.legacy.eng.json`);
    try {
      const sd = legacyShowdown(cand, timeoutMs);
      if (sd.status !== 0 || sd.signal !== null) return {success: false};
      let sdOut; try { sdOut = JSON.parse(sd.stdout || ''); } catch (_) { return {success: false}; }
      if (sdOut.success === false || sdOut.error === 'Not all choices done') return {success: false};
      const eng = legacyEngine(cand, timeoutMs);
      if (eng.status !== 0 || eng.signal !== null) return {success: false};
      let engOut; try { engOut = JSON.parse(eng.stdout || ''); } catch (_) { return {success: false}; }
      fs.writeFileSync(sdT, sd.stdout);
      fs.writeFileSync(engT, eng.stdout);
      const cmp = legacyComparator(sdT, engT, timeoutMs);
      let cmpOut; try { cmpOut = JSON.parse(cmp.stdout || ''); } catch (_) { cmpOut = null; }
      const cmpDiffs = (cmpOut && Array.isArray(cmpOut.diffs)) ? cmpOut.diffs : [];
      let dt = null;
      for (const d of cmpDiffs) {
        if (!d || typeof d.path !== 'string') continue;
        const m = d.path.match(/^turns\[(\d+)\]\./);
        if (!m) continue;
        const tt = parseInt(m[1], 10);
        if (dt === null || tt < dt) dt = tt;
      }
      return {
        success: true,
        signature: minimize.buildSignature(
          Array.isArray(engOut.turns) ? engOut.turns : [],
          cmpDiffs, cand,
        ),
        diffs: cmpDiffs,
        divergent_turn: dt,
        turn_results: Array.isArray(engOut.turns) ? engOut.turns : [],
      };
    } finally {
      try { fs.unlinkSync(sdT); } catch (_) {}
      try { fs.unlinkSync(engT); } catch (_) {}
    }
  };
}

function makeServerOracle(enginePool, sdPool, timeoutMs) {
  const tmpDir = path.join(__dirname, 'parity_tmp');
  fs.mkdirSync(tmpDir, {recursive: true});
  return async (cand) => {
    const sdT = path.join(tmpDir, `oracle.${process.pid}.server.sd.json`);
    const engT = path.join(tmpDir, `oracle.${process.pid}.server.eng.json`);
    try {
      let sd, eng;
      try { sd = await sdPool.runScenario(cand, timeoutMs); } catch (_) { return {success: false}; }
      if (sd.respawned || sd.signal !== null || sd.status !== 0) return {success: false};
      let sdOut; try { sdOut = JSON.parse(sd.stdout || ''); } catch (_) { return {success: false}; }
      if (sdOut.success === false || sdOut.error === 'Not all choices done') return {success: false};
      try { eng = await enginePool.runScenario(cand, timeoutMs); } catch (_) { return {success: false}; }
      if (eng.respawned || eng.signal !== null || eng.status !== 0) return {success: false};
      let engOut; try { engOut = JSON.parse(eng.stdout || ''); } catch (_) { return {success: false}; }
      fs.writeFileSync(sdT, sd.stdout);
      fs.writeFileSync(engT, eng.stdout);
      const cmp = legacyComparator(sdT, engT, timeoutMs);
      let cmpOut; try { cmpOut = JSON.parse(cmp.stdout || ''); } catch (_) { cmpOut = null; }
      const cmpDiffs = (cmpOut && Array.isArray(cmpOut.diffs)) ? cmpOut.diffs : [];
      let dt = null;
      for (const d of cmpDiffs) {
        if (!d || typeof d.path !== 'string') continue;
        const m = d.path.match(/^turns\[(\d+)\]\./);
        if (!m) continue;
        const tt = parseInt(m[1], 10);
        if (dt === null || tt < dt) dt = tt;
      }
      return {
        success: true,
        signature: minimize.buildSignature(
          Array.isArray(engOut.turns) ? engOut.turns : [],
          cmpDiffs, cand,
        ),
        diffs: cmpDiffs,
        divergent_turn: dt,
        turn_results: Array.isArray(engOut.turns) ? engOut.turns : [],
      };
    } finally {
      try { fs.unlinkSync(sdT); } catch (_) {}
      try { fs.unlinkSync(engT); } catch (_) {}
    }
  };
}

function canonicalScenario(s) { return canonical(s); }
function canonicalSignature(sig) {
  const arr = Array.isArray(sig) ? sig.slice().sort() : Array.from(sig || []).sort();
  return canonical(arr);
}
function hashStr(s) {
  return crypto.createHash('sha256').update(s).digest('hex');
}

async function runMinimizerSetting(label, fixture, oracle, repeats, config) {
  // Rebuild the baseline identically for every setting, or the comparison is not parity.
  const origDiff = {diffs: fixture.diffs, divergent_turn: fixture.divergent_turn};
  const origSig = new Set(fixture.signature);
  const out = [];
  for (let i = 0; i < repeats; i++) {
    let mr;
    try {
      mr = await minimize.minimize(fixture.scenario, origDiff, origSig, config, oracle);
    } catch (e) {
      out.push({error: e.message || String(e)});
      continue;
    }
    out.push({
      scenarioHash: hashStr(canonicalScenario(mr.scenario)),
      signatureHash: hashStr(canonicalSignature(mr.signature)),
      verified: !!mr.minimize_verified,
      turns: mr.scenario && mr.scenario.turns ? mr.scenario.turns.length : 0,
      sigLen: Array.isArray(mr.signature) ? mr.signature.length : 0,
    });
  }
  return out;
}

async function runPass4(targetFixtures) {
  console.log(`\n=== Pass-4: minimizer end-to-end (target ${targetFixtures} fixtures) ===`);
  const {fixtures, scanned} = await discoverDivergentFixtures(targetFixtures, 0);
  console.log(`Discovered ${fixtures.length} divergent fixtures (scanned ${scanned} seeds).`);
  if (fixtures.length < targetFixtures) {
    console.error(`pass-4: insufficient fixtures (${fixtures.length} < ${targetFixtures}); ` +
                  `bisect by re-running with --pass4-fixtures ${fixtures.length} or scanning more seeds.`);
    return {pass: 'pass4', passed: false, reason: 'insufficient_fixtures', fixturesFound: fixtures.length};
  }

  const minConfig = {minimize_budget_seconds: ARGV.minimizeBudgetSeconds};
  const repeats = 3;
  const settings = [
    {key: 'legacy', recycleAfter: null},
    {key: 'server-r1000', recycleAfter: 1000},
    {key: 'server-r1', recycleAfter: 1},
  ];

  const perFixture = [];
  let mismatches = 0;
  let firstMismatch = null;

  for (let fi = 0; fi < fixtures.length; fi++) {
    const fixture = fixtures[fi];
    const fxOut = {seed: fixture.seed, turns: fixture.scenario.turns.length, settings: {}};
    for (const setting of settings) {
      let oracle;
      let enginePool, sdPool;
      if (setting.key === 'legacy') {
        oracle = makeLegacyOracle(ARGV.scenarioTimeoutMs);
      } else {
        enginePool = new SubprocessPool({
          command: ENGINE_BIN, args: ['--server-mode'],
          label: `eng-${setting.key}`, kind: 'engine', recycleAfter: setting.recycleAfter,
        });
        sdPool = new SubprocessPool({
          command: 'node', args: [SHOWDOWN_RUNNER, '--server-mode'],
          label: `sd-${setting.key}`, kind: 'showdown', recycleAfter: setting.recycleAfter,
        });
        oracle = makeServerOracle(enginePool, sdPool, ARGV.scenarioTimeoutMs);
      }
      try {
        const repeatsOut = await runMinimizerSetting(
          setting.key, fixture, oracle, repeats, minConfig
        );
        fxOut.settings[setting.key] = repeatsOut;
      } finally {
        if (enginePool) await enginePool.shutdown();
        if (sdPool) await sdPool.shutdown();
      }
    }

    // Every repeat and setting must produce an identical scenarioHash and signatureHash.
    const allHashes = [];
    for (const setting of settings) {
      const reps = fxOut.settings[setting.key] || [];
      for (let ri = 0; ri < reps.length; ri++) {
        const r = reps[ri];
        if (r.error) {
          allHashes.push({key: setting.key, rep: ri, scenarioHash: 'ERR', signatureHash: 'ERR', err: r.error});
        } else {
          allHashes.push({key: setting.key, rep: ri, scenarioHash: r.scenarioHash, signatureHash: r.signatureHash});
        }
      }
    }
    const refScenario = allHashes[0].scenarioHash;
    const refSignature = allHashes[0].signatureHash;
    let fixtureMismatch = false;
    for (const h of allHashes) {
      if (h.scenarioHash !== refScenario || h.signatureHash !== refSignature) {
        fixtureMismatch = true;
        break;
      }
    }
    fxOut.allHashes = allHashes;
    fxOut.match = !fixtureMismatch;
    if (fixtureMismatch) {
      mismatches++;
      if (!firstMismatch) firstMismatch = fxOut;
    }
    perFixture.push(fxOut);
    console.log(`  fixture ${fi + 1}/${fixtures.length} seed=${fixture.seed}: ` +
                `${fixtureMismatch ? 'MISMATCH' : 'match'}`);
  }

  return {
    pass: 'pass4',
    passed: mismatches === 0,
    fixturesEvaluated: fixtures.length,
    mismatches,
    firstMismatch,
    perFixture,
  };
}

async function main() {
  if (!fs.existsSync(ENGINE_BIN)) {
    console.error(`parity_check: engine binary not found at ${ENGINE_BIN}.`);
    console.error('Build with: cd conformance/harness/run_scenario && cargo build');
    process.exit(2);
  }

  const isSmoke = ARGV.smoke;
  const seedsA = ARGV.seeds !== null ? ARGV.seeds : (isSmoke ? 10 : 10000);
  const seedsB = ARGV.seeds !== null ? ARGV.seeds : (isSmoke ? 10 : 10000);
  const seedsC = ARGV.seeds !== null ? Math.min(ARGV.seeds, 1000) : (isSmoke ? 5 : 1000);
  const pass4N = isSmoke ? Math.min(ARGV.pass4Fixtures, 3) : ARGV.pass4Fixtures;

  const report = {
    startedAt: new Date().toISOString(),
    host: require('os').hostname(),
    smoke: isSmoke,
    args: ARGV,
    preflights: null,
    passes: {},
  };

  const requested = (label) => ARGV.pass === 'all' || ARGV.pass === label;

  if (requested('preflights') || ARGV.pass === 'all') {
    console.log('=== Pre-flights ===');
    const cfg = {max_turns_per_scenario: 4, rng_mode_split: {force_all: 1.0}};
    const buildPool = new SubprocessPool({
      command: ENGINE_BIN, args: ['--server-mode'],
      label: 'engine-pre', kind: 'engine', recycleAfter: Infinity,
    });
    let scenarioObj;
    try {
      scenarioObj = await buildScenario(0, cfg, buildPool, 4);
    } finally {
      await buildPool.shutdown();
    }

    const sPool = new SubprocessPool({
      command: ENGINE_BIN, args: ['--server-mode'],
      label: 'engine-canary', kind: 'engine', recycleAfter: Infinity,
    });
    let serverEng;
    try { serverEng = await sPool.runScenario(scenarioObj, ARGV.scenarioTimeoutMs); }
    finally { await sPool.shutdown(); }
    const serverHash = parityHashOfStdout(serverEng.stdout || '');

    const canaryOk = await preflightCanaryInversion(JSON.parse(serverEng.stdout), serverHash);
    const selfVal = await preflightSelfValidation(scenarioObj, ARGV.scenarioTimeoutMs);
    report.preflights = {
      canaryInversion: {ok: canaryOk},
      selfValidation: selfVal,
    };
    console.log(`  canary-inversion:  ${canaryOk ? 'OK' : 'FAIL'}`);
    console.log(`  self-validation:   ${selfVal.ok ? 'OK' : 'FAIL'} (${selfVal.reason})`);
    if (!canaryOk || !selfVal.ok) {
      console.error('parity_check: pre-flight FAILED — aborting before sweep.');
      report.aborted = 'preflights_failed';
      fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
      process.exit(1);
    }
    if (ARGV.pass === 'preflights') {
      report.endedAt = new Date().toISOString();
      fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
      console.log('Pre-flights only: complete.');
      return;
    }
  }

  if (requested('A') || ARGV.pass === 'all') {
    console.log(`\n=== Pass A: recycleAfter=Infinity, ${seedsA} seeds (Dex-stratified) ===`);
    report.passes.A = await runOnePassEngine('A', seedsA, Infinity, ARGV.startSeed,
                                              isSmoke ? 5 : 250);
    fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
    console.log(`  passA: evaluated=${report.passes.A.evaluated} ` +
                `engDiv=${report.passes.A.engineDiverged} sdDiv=${report.passes.A.showdownDiverged} ` +
                `passed=${report.passes.A.passed} (${report.passes.A.elapsedSec.toFixed(1)}s)`);
  }

  if (requested('B') || ARGV.pass === 'all') {
    console.log(`\n=== Pass B: recycleAfter=1000, ${seedsB} seeds (Dex-stratified) ===`);
    report.passes.B = await runOnePassEngine('B', seedsB, isSmoke ? 5 : 1000, ARGV.startSeed + 100000,
                                              isSmoke ? 5 : 250);
    fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
    console.log(`  passB: evaluated=${report.passes.B.evaluated} ` +
                `engDiv=${report.passes.B.engineDiverged} sdDiv=${report.passes.B.showdownDiverged} ` +
                `passed=${report.passes.B.passed} (${report.passes.B.elapsedSec.toFixed(1)}s)`);
  }

  if (requested('C') || ARGV.pass === 'all') {
    console.log(`\n=== Pass C: recycleAfter=100, ${seedsC} seeds (Dex-stratified) ===`);
    report.passes.C = await runOnePassEngine('C', seedsC, isSmoke ? 3 : 100, ARGV.startSeed + 200000,
                                              isSmoke ? 5 : 100);
    fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
    console.log(`  passC: evaluated=${report.passes.C.evaluated} ` +
                `engDiv=${report.passes.C.engineDiverged} sdDiv=${report.passes.C.showdownDiverged} ` +
                `passed=${report.passes.C.passed} (${report.passes.C.elapsedSec.toFixed(1)}s)`);
  }

  // Three-pass gate: must pass A, B, C before pass-4.
  const threePassPassed = ['A', 'B', 'C'].every(
    (k) => !report.passes[k] || report.passes[k].passed
  );

  if ((requested('pass4') || ARGV.pass === 'all') && threePassPassed) {
    report.passes.pass4 = await runPass4(pass4N);
    fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
    console.log(`  pass4: ${report.passes.pass4.passed ? 'PASS' : 'FAIL'} ` +
                `(mismatches=${report.passes.pass4.mismatches})`);
  } else if (!threePassPassed && (requested('pass4') || ARGV.pass === 'all')) {
    console.log('\n=== Pass-4 SKIPPED: three-pass gate not green ===');
    report.passes.pass4 = {skipped: true, reason: 'three_pass_failed'};
  }

  report.endedAt = new Date().toISOString();
  const allPassed = ['A', 'B', 'C'].every((k) => !report.passes[k] || report.passes[k].passed)
                    && (!report.passes.pass4 || report.passes.pass4.passed === true || report.passes.pass4.skipped);
  report.verdict = allPassed ? 'PASS' : 'FAIL';
  fs.writeFileSync(REPORT_PATH, JSON.stringify(report, null, 2));
  console.log(`\n=== parity_check verdict: ${report.verdict} ===`);
  console.log(`Report: ${REPORT_PATH}`);
  process.exit(allPassed ? 0 : 1);
}

main().catch((e) => {
  console.error(`parity_check: top-level failure: ${e && (e.stack || e.message) || e}`);
  process.exit(2);
});
