#!/usr/bin/env node
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const FUZZER_DIR = path.join(__dirname, '..');
const HARNESS_DIR = path.join(FUZZER_DIR, '..', 'harness');
const ENGINE_BIN = path.join(HARNESS_DIR, 'run_scenario', 'target', 'debug', 'run_scenario');
const SHOWDOWN_RUNNER = path.join(HARNESS_DIR, 'showdown_runner.js');
const COMPARE_RESULTS = path.join(HARNESS_DIR, 'compare_results.js');
const CONFIG_PATH = path.join(FUZZER_DIR, 'config.json');

const { SubprocessPool } = require(path.join(FUZZER_DIR, 'lib', 'subprocess_pool.js'));
const generate = require(path.join(FUZZER_DIR, 'generate.js'));
const { compareResults, loadFormeMap } = require(COMPARE_RESULTS);

const COUNT = (() => {
  const i = process.argv.indexOf('--count');
  return i !== -1 ? parseInt(process.argv[i + 1], 10) : 1000;
})();

function cliCompare(tmpDir, sdStdout, engStdout) {
  const sdT = path.join(tmpDir, 'sd.json');
  const engT = path.join(tmpDir, 'eng.json');
  fs.writeFileSync(sdT, sdStdout);
  fs.writeFileSync(engT, engStdout);
  const r = spawnSync('node', [COMPARE_RESULTS, sdT, engT], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  return { status: r.status, stdout: r.stdout || '' };
}

async function main() {
  const config = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'));
  const formeMap = loadFormeMap();
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'cmpparity-'));

  const enginePool = new SubprocessPool({ command: ENGINE_BIN, args: ['--server-mode'], label: 'engine', kind: 'engine', recycleAfter: 100000 });
  const showdownPool = new SubprocessPool({ command: 'node', args: [SHOWDOWN_RUNNER, '--server-mode'], label: 'showdown', kind: 'showdown', recycleAfter: 100000 });

  let compared = 0, matched = 0, skipped = 0;
  const failures = [];

  const checkPair = (label, sdStdout, engStdout) => {
    let inProc, threw = null;
    try { inProc = compareResults(JSON.parse(sdStdout), JSON.parse(engStdout), formeMap); }
    catch (e) { threw = e; }
    const cli = cliCompare(tmpDir, sdStdout, engStdout);
    compared++;
    if (threw) {
      // In-process threw → CLI must also have failed (non-zero, unparseable).
      let cliParsed = null;
      try { cliParsed = JSON.parse(cli.stdout); } catch (_) {}
      if (cli.status !== 0 && cliParsed === null) { matched++; return { throwAsym: true }; }
      failures.push(`${label}: in-process threw (${threw.message}) but CLI status=${cli.status} parseable=${cliParsed !== null}`);
      return { throwAsym: false };
    }
    const want = cli.stdout.replace(/\n+$/, '');
    const got = JSON.stringify(inProc, null, 2);
    const statusOk = cli.status === (inProc.verdict === 'PASS' ? 0 : 1);
    if (got === want && statusOk) { matched++; return { ok: true, verdict: inProc.verdict }; }
    if (failures.length < 10) {
      failures.push(`${label}: byte/STATUS mismatch (statusOk=${statusOk})\n  GOT:  ${got.slice(0, 300)}\n  WANT: ${want.slice(0, 300)}`);
    }
    return { ok: false, verdict: inProc.verdict };
  };

  let divergedSeen = 0, passSeen = 0;
  for (let seed = 0; seed < COUNT; seed++) {
    let scenario;
    try { scenario = (await generate.generateScenario(seed, config, enginePool)).scenario; }
    catch (_) { skipped++; continue; }
    let sd, eng;
    try { sd = await showdownPool.runScenario(scenario, 30000); } catch (e) { sd = e; }
    if (!sd || sd.respawned || sd.signal !== null || typeof sd.stdout !== 'string') { skipped++; continue; }
    let sdOut; try { sdOut = JSON.parse(sd.stdout); } catch (_) { skipped++; continue; }
    if (sdOut.success === false || sdOut.error === 'Not all choices done') { skipped++; continue; }
    try { eng = await enginePool.runScenario(scenario, 30000); } catch (e) { eng = e; }
    if (!eng || eng.respawned || eng.signal !== null || eng.status !== 0 || typeof eng.stdout !== 'string') { skipped++; continue; }
    const res = checkPair(`seed=${seed}`, sd.stdout, eng.stdout);
    if (res && res.verdict === 'FAIL') divergedSeen++;
    else if (res && res.verdict === 'PASS') passSeen++;
  }

  checkPair('edge:calc_damage_match', JSON.stringify({
    name: 'cd', mode: 'calc_damage',
    results: { all_rolls: [{ damage: 10 }, { damage: 12 }], min_damage: 10, max_damage: 12, crit_results: [{ damage: 20 }] },
  }), JSON.stringify({
    name: 'cd', mode: 'calc_damage',
    results: { all_rolls: [{ damage: 12 }, { damage: 10 }], min_damage: 10, max_damage: 12, crit_results: [{ damage: 20 }] },
  }));
  checkPair('edge:calc_damage_diff', JSON.stringify({
    name: 'cd', mode: 'calc_damage',
    results: { all_rolls: [{ damage: 10 }], min_damage: 10, max_damage: 10, crit_results: [{ damage: 20 }] },
  }), JSON.stringify({
    name: 'cd', mode: 'calc_damage',
    results: { all_rolls: [{ damage: 11 }], min_damage: 11, max_damage: 11, crit_results: [{ damage: 21 }] },
  }));

  checkPair('edge:not_all_choices_done', JSON.stringify({
    name: 'nac', mode: 'execute_turns', success: false, error: 'Not all choices done',
  }), JSON.stringify({
    name: 'nac', mode: 'execute_turns', turns: [],
  }));

  // The CLI tolerates a null turn (exit 1, unparseable stdout) where the in-process call throws.
  const asym = checkPair('edge:null_turn_malformed', JSON.stringify({
    name: 'bad', mode: 'execute_turns', turns: [null],
  }), JSON.stringify({
    name: 'bad', mode: 'execute_turns', turns: [null],
  }));

  await Promise.allSettled([enginePool.shutdown(), showdownPool.shutdown()]);
  fs.rmSync(tmpDir, { recursive: true, force: true });

  console.log(`\ncompared=${compared} matched=${matched} skipped=${skipped} ` +
              `(bulk verdict mix: PASS=${passSeen} FAIL=${divergedSeen})`);
  console.log(`null-turn throw asymmetry confirmed: ${asym && asym.throwAsym === true}`);
  if (failures.length) {
    console.error(`\nFAILURES (${failures.length}):`);
    for (const f of failures) console.error('  ' + f);
    process.exit(1);
  }
  if (matched !== compared) { console.error('matched != compared'); process.exit(1); }
  if (divergedSeen === 0 || passSeen === 0) {
    console.error('WARNING: bulk corpus did not include both PASS and FAIL verdicts — coverage weak');
  }
  console.log('\nPHASE-1 BYTE-PARITY: PASS');
  process.exit(0);
}

main().catch((e) => { console.error(e); process.exit(1); });
