'use strict';

// Set before pools drain, so workers stop scheduling and the drain's rejections never reach stats.jsonl.

let halted = false;
let reason = null;

module.exports = {
  isHalted() { return halted; },
  halt(r) { halted = true; if (reason === null) reason = r || 'halt'; },
  reason() { return reason; },
  // test-only
  _reset() { halted = false; reason = null; },
};
