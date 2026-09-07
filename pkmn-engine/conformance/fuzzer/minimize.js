'use strict';

const { path_to_category, SIGNATURE_CATEGORIES_SET } = require('./constants.js');

// Engine-space move id, not a Showdown one.
const SPLASH_MOVE_ID = 150;

// Action bytes: 0-3 are move slots, 4-9 switch to team slot (byte - 4), 10 is tera, 255 is struggle.
const SWITCH_BYTE_LO = 4;
const SWITCH_BYTE_HI = 9;

// last_move survives the end-of-turn clear, so the recharging guard is what marks a real move.
function buildSideTuple(state_after_side, action_byte) {
  const active = state_after_side && state_after_side.active;
  const team = (state_after_side && state_after_side.team) || [];
  const active_index = state_after_side ? state_after_side.active_index : 0;
  const active_mon = team[active_index] || {};

  const last_move = active && typeof active.last_move === 'number' ? active.last_move : 0;
  const vols = (active && Array.isArray(active.volatile_names)) ? active.volatile_names : [];
  const recharging = vols.indexOf('recharging') !== -1;
  const did_move = last_move !== 0 && !recharging;
  const effective_move_id_if_moved = did_move ? last_move : 0;

  return [
    active_mon.species_id || 0,
    active_mon.ability_id || 0,
    active_mon.item_id || 0,
    action_byte | 0,
    effective_move_id_if_moved,
    did_move ? 1 : 0,
  ];
}

function buildSignature(turn_results, diffs, scenario) {
  const sig = new Set();
  if (!Array.isArray(diffs)) return sig;

  for (const diff of diffs) {
    if (!diff || typeof diff.path !== 'string') continue;

    const m = diff.path.match(/^turns\[(\d+)\]\./);
    if (!m) continue;
    const turn_index = parseInt(m[1], 10);

    const cat = path_to_category(diff.path, diff.showdown, diff.engine);
    // path_to_category returns an array for the argless HP case, a string with both operands.
    const category = Array.isArray(cat) ? cat[0] : cat;

    const tr = turn_results && turn_results[turn_index];
    const sa = tr && tr.state_after ? tr.state_after : null;
    const turn = scenario && scenario.turns && scenario.turns[turn_index];
    const p1_byte = turn ? turn.p1_action : 0;
    const p2_byte = turn ? turn.p2_action : 0;

    const p1_tuple = buildSideTuple(sa ? sa.p1 : null, p1_byte);
    const p2_tuple = buildSideTuple(sa ? sa.p2 : null, p2_byte);

    sig.add(JSON.stringify([turn_index, p1_tuple, p2_tuple, category]));
  }
  return sig;
}

function setsEqual(a, b) {
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}

function isSubset(small, big) {
  for (const v of small) if (!big.has(v)) return false;
  return true;
}

function rewriteForSlotK(sig, side, slotK, scenario, turn_results) {
  const out = new Set();
  for (const key of sig) {
    const tup = JSON.parse(key);
    const turn_index = tup[0];
    const turn = scenario && scenario.turns && scenario.turns[turn_index];
    const sideByte = turn ? (side === 'p1' ? turn.p1_action : turn.p2_action) : 0;
    const tr = turn_results && turn_results[turn_index];
    const sa = tr && tr.state_after ? tr.state_after : null;
    const sideSnap = sa ? sa[side] : null;
    const active_index = sideSnap ? sideSnap.active_index : -1;

    // The signature key carries no team-slot info, so neutralise whenever the byte equals slotK.
    if (sideByte === slotK) {
      const idx = side === 'p1' ? 1 : 2;
      const tuple = tup[idx].slice();
      tuple[4] = 0; // effective_move_id_if_moved → neutral
      tup[idx] = tuple;
    }
    out.add(JSON.stringify(tup));
  }
  return out;
}

function signaturesEqualExcludingSlotK(before, after, side, slotK, scenarioBefore, scenarioAfter, trBefore, trAfter) {
  const a = rewriteForSlotK(before, side, slotK, scenarioBefore, trBefore);
  const b = rewriteForSlotK(after, side, slotK, scenarioAfter, trAfter);
  return setsEqual(a, b);
}

async function runAndSign(runOracle, scenario) {
  const r = await runOracle(scenario);
  if (!r || r.success === false) return null;
  const sig = buildSignature(r.turn_results || [], r.diffs || [], scenario);
  return { result: r, signature: sig };
}

function clone(x) { return JSON.parse(JSON.stringify(x)); }

function referencedTeamSlots(scenario, side) {
  const refs = new Set();
  // The starting active is always slot 0; never drop it.
  refs.add(0);
  const turns = (scenario && scenario.turns) || [];
  const key = side === 'p1' ? 'p1_action' : 'p2_action';
  for (const t of turns) {
    const b = t[key];
    if (typeof b !== 'number') continue;
    if (b >= SWITCH_BYTE_LO && b <= SWITCH_BYTE_HI) {
      refs.add(b - SWITCH_BYTE_LO);
    }
  }
  return refs;
}

async function step1_turnCount(scenario, baselineSig, runOracle, deadline) {
  let current = scenario;
  let currentSig = baselineSig;
  let accepted = false;

  while (Date.now() < deadline && current.turns && current.turns.length > 1) {
    const half = Math.floor(current.turns.length / 2);
    if (half < 1) break;
    const cand = clone(current);
    cand.turns = cand.turns.slice(0, half);
    const r = await runAndSign(runOracle, cand);
    if (r && setsEqual(r.signature, currentSig)) {
      current = cand;
      currentSig = r.signature;
      accepted = true;
    } else {
      break;
    }
  }

  while (Date.now() < deadline && current.turns && current.turns.length > 1) {
    const cand = clone(current);
    cand.turns = cand.turns.slice(0, current.turns.length - 1);
    const r = await runAndSign(runOracle, cand);
    if (r && setsEqual(r.signature, currentSig)) {
      current = cand;
      currentSig = r.signature;
      accepted = true;
    } else {
      break;
    }
  }

  return { scenario: current, signature: currentSig, accepted };
}

async function step2_teamSize(scenario, baselineSig, runOracle, deadline) {
  let current = scenario;
  let currentSig = baselineSig;
  let accepted = false;

  for (const side of ['p1', 'p2']) {
    if (Date.now() >= deadline) break;
    let team = (current.teams && current.teams[side]) || [];
    let i = team.length - 1;
    while (i >= 1 && Date.now() < deadline) {
      const refs = referencedTeamSlots(current, side);
      if (refs.has(i)) { i--; continue; }
      const cand = clone(current);
      cand.teams[side] = cand.teams[side].slice(0, i).concat(cand.teams[side].slice(i + 1));
      const r = await runAndSign(runOracle, cand);
      if (r && setsEqual(r.signature, currentSig)) {
        current = cand;
        currentSig = r.signature;
        accepted = true;
        team = current.teams[side];
        i = team.length - 1;
      } else {
        i--;
      }
    }
  }

  return { scenario: current, signature: currentSig, accepted };
}

// Signatures compare modulo slot-K's move id, but any change to the category set rejects the shrink.
async function step3_splash(scenario, baselineSig, runOracle, deadline, baselineTurnResults) {
  let current = scenario;
  let currentSig = baselineSig;
  let currentTr = baselineTurnResults;
  let accepted = false;

  for (const side of ['p1', 'p2']) {
    const team = (current.teams && current.teams[side]) || [];
    for (let monIdx = 0; monIdx < team.length; monIdx++) {
      for (let slotK = 0; slotK < 4; slotK++) {
        if (Date.now() >= deadline) break;
        const beforeSlot = current.teams[side][monIdx].moves[slotK];
        if (beforeSlot === SPLASH_MOVE_ID || beforeSlot === 0) continue;
        const cand = clone(current);
        cand.teams[side][monIdx].moves[slotK] = SPLASH_MOVE_ID;
        const r = await runAndSign(runOracle, cand);
        if (!r) continue;
        const eq = signaturesEqualExcludingSlotK(
          currentSig, r.signature, side, slotK,
          current, cand, currentTr, r.result.turn_results,
        );
        if (eq) {
          current = cand;
          currentSig = r.signature;
          currentTr = r.result.turn_results;
          accepted = true;
        }
      }
    }
  }

  return { scenario: current, signature: currentSig, turn_results: currentTr, accepted };
}

// Clear only: substituting an item would change the bug's repro shape.
async function step4_itemClear(scenario, baselineSig, runOracle, deadline) {
  let current = scenario;
  let currentSig = baselineSig;
  let accepted = false;

  for (const side of ['p1', 'p2']) {
    const team = (current.teams && current.teams[side]) || [];
    for (let monIdx = 0; monIdx < team.length; monIdx++) {
      if (Date.now() >= deadline) break;
      const mon = current.teams[side][monIdx];
      if (!mon || !mon.item_id) continue;
      const cand = clone(current);
      cand.teams[side][monIdx].item_id = 0;
      const r = await runAndSign(runOracle, cand);
      if (r && setsEqual(r.signature, currentSig)) {
        current = cand;
        currentSig = r.signature;
        accepted = true;
      }
    }
  }

  return { scenario: current, signature: currentSig, accepted };
}

// Whole blocks only: sub-keys deliberately collapse with their parent, matching steps 2 and 3.
async function step5_stateOverrides(scenario, baselineSig, runOracle, deadline) {
  let current = scenario;
  let currentSig = baselineSig;
  let accepted = false;

  if (!current.state_overrides) {
    return { scenario: current, signature: currentSig, accepted };
  }

  const tryStrip = async (mutator) => {
    if (Date.now() >= deadline) return;
    const cand = clone(current);
    if (!cand.state_overrides) return;
    if (!mutator(cand.state_overrides)) return;
    const r = await runAndSign(runOracle, cand);
    if (r && setsEqual(r.signature, currentSig)) {
      current = cand;
      currentSig = r.signature;
      accepted = true;
    }
  };

  await tryStrip(so => {
    if (so.field === undefined) return false;
    delete so.field;
    return true;
  });

  for (const side of ['p1', 'p2']) {
    await tryStrip(so => {
      if (!so[side] || so[side].side_conditions === undefined) return false;
      delete so[side].side_conditions;
      return true;
    });
    await tryStrip(so => {
      if (!so[side] || so[side].team_overrides === undefined) return false;
      delete so[side].team_overrides;
      return true;
    });
    await tryStrip(so => {
      if (!so[side] || so[side].active_overrides === undefined) return false;
      delete so[side].active_overrides;
      return true;
    });
  }

  return { scenario: current, signature: currentSig, accepted };
}

async function minimize(scenario, originalDiff, originalSignature, config, runOracle) {
  const budgetMs = ((config && config.minimize_budget_seconds) || 15) * 1000;
  const deadline = Date.now() + budgetMs;

  // originalSignature may arrive as a Set, an Array, or undefined.
  let originalSig;
  if (originalSignature instanceof Set) {
    originalSig = originalSignature;
  } else if (Array.isArray(originalSignature)) {
    originalSig = new Set(originalSignature);
  } else {
    originalSig = new Set();
  }

  let originalDivergentTurn = null;
  if (originalDiff && Array.isArray(originalDiff.diffs)) {
    for (const d of originalDiff.diffs) {
      if (!d || typeof d.path !== 'string') continue;
      const m = d.path.match(/^turns\[(\d+)\]\./);
      if (m) {
        const t = parseInt(m[1], 10);
        if (originalDivergentTurn === null || t < originalDivergentTurn) {
          originalDivergentTurn = t;
        }
      }
    }
  } else if (originalDiff && typeof originalDiff.divergent_turn === 'number') {
    originalDivergentTurn = originalDiff.divergent_turn;
  }

  // Re-run the original to ground turn_results for step 3 and confirm the signature reproduces.
  const baselineRun = await runAndSign(runOracle, scenario);
  if (!baselineRun || baselineRun.signature.size === 0) {
    return {
      scenario,
      signature: Array.from(originalSig),
      minimize_verified: false,
      first_divergent_turn_preserved: false,
    };
  }

  let currentScenario = scenario;
  let currentSig = baselineRun.signature;
  let currentTr = baselineRun.result.turn_results;
  let bestScenario = currentScenario;
  let bestSig = currentSig;

  let totalAccepts = 0;
  let firstPassAccepted = false;

  while (Date.now() < deadline) {
    let passAccepted = false;

    {
      const out = await step1_turnCount(currentScenario, currentSig, runOracle, deadline);
      if (out.accepted) {
        currentScenario = out.scenario;
        currentSig = out.signature;
        passAccepted = true;
        // Refresh turn_results so step 3's slot-K rewrite sees the shrunk scenario.
        const r = await runAndSign(runOracle, currentScenario);
        if (r) currentTr = r.result.turn_results;
      }
    }
    if (Date.now() >= deadline) break;

    {
      const out = await step2_teamSize(currentScenario, currentSig, runOracle, deadline);
      if (out.accepted) {
        currentScenario = out.scenario;
        currentSig = out.signature;
        passAccepted = true;
        const r = await runAndSign(runOracle, currentScenario);
        if (r) currentTr = r.result.turn_results;
      }
    }
    if (Date.now() >= deadline) break;

    {
      const out = await step3_splash(currentScenario, currentSig, runOracle, deadline, currentTr);
      if (out.accepted) {
        currentScenario = out.scenario;
        currentSig = out.signature;
        currentTr = out.turn_results;
        passAccepted = true;
      }
    }
    if (Date.now() >= deadline) break;

    {
      const out = await step4_itemClear(currentScenario, currentSig, runOracle, deadline);
      if (out.accepted) {
        currentScenario = out.scenario;
        currentSig = out.signature;
        passAccepted = true;
        const r = await runAndSign(runOracle, currentScenario);
        if (r) currentTr = r.result.turn_results;
      }
    }
    if (Date.now() >= deadline) break;

    {
      const out = await step5_stateOverrides(currentScenario, currentSig, runOracle, deadline);
      if (out.accepted) {
        currentScenario = out.scenario;
        currentSig = out.signature;
        passAccepted = true;
        const r = await runAndSign(runOracle, currentScenario);
        if (r) currentTr = r.result.turn_results;
      }
    }

    if (passAccepted) {
      totalAccepts++;
      firstPassAccepted = true;
      bestScenario = currentScenario;
      bestSig = currentSig;
    } else {
      break;
    }
  }

  if (!firstPassAccepted) {
    return {
      scenario,
      signature: Array.from(originalSig),
      minimize_verified: false,
      first_divergent_turn_preserved: false,
    };
  }

  // Wallclock trip: emit best-so-far unverified, skipping the end-of-minimization checks.
  if (Date.now() >= deadline) {
    return {
      scenario: bestScenario,
      signature: Array.from(bestSig).sort(),
      minimize_verified: false,
      first_divergent_turn_preserved: false,
    };
  }

  const reRun = await runAndSign(runOracle, bestScenario);

  let minimize_verified = true;
  if (!reRun) {
    minimize_verified = false;
  } else if (reRun.signature.size === 0) {
    // An empty signature is a subset of anything, so a shrink that ate the failure would pass.
    minimize_verified = false;
  } else if (!isSubset(reRun.signature, originalSig)) {
    minimize_verified = false;
  }

  let first_divergent_turn_preserved = false;
  if (reRun && originalDivergentTurn !== null) {
    // The turn survives only if it is still inside the shrunk list and still the earliest diff.
    if (bestScenario.turns && originalDivergentTurn < bestScenario.turns.length) {
      let minDivergent = null;
      for (const d of (reRun.result.diffs || [])) {
        if (!d || typeof d.path !== 'string') continue;
        const mm = d.path.match(/^turns\[(\d+)\]\./);
        if (mm) {
          const t = parseInt(mm[1], 10);
          if (minDivergent === null || t < minDivergent) minDivergent = t;
        }
      }
      if (minDivergent === originalDivergentTurn) {
        first_divergent_turn_preserved = true;
      }
    }
  }

  if (!minimize_verified || !first_divergent_turn_preserved) {
    return {
      scenario,
      signature: Array.from(originalSig),
      minimize_verified: false,
      first_divergent_turn_preserved,
    };
  }

  return {
    scenario: bestScenario,
    signature: Array.from(reRun.signature).sort(),
    minimize_verified: true,
    first_divergent_turn_preserved: true,
  };
}

module.exports = {
  minimize,
  buildSignature,
  // Exported for unit tests only; the driver does not consume these.
  _internal: {
    buildSideTuple,
    setsEqual,
    isSubset,
    referencedTeamSlots,
    signaturesEqualExcludingSlotK,
  },
};
