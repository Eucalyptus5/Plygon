'use strict';

// In the minimizer's array-shaped side tuple, index 4 is effective_move_id_if_moved.

const SPLASH_MOVE_ID = 150;

function signatureMentionsSplash(signature) {
  if (!signature) return false;
  const arr = Array.isArray(signature) ? signature : Array.from(signature);
  for (const sig of arr) {
    const ent = sig && (sig.entity_id_set_at_turn || sig.entity_id_set || sig[1]);
    if (!ent) continue;
    for (const sideKey of ['p1', 'p2']) {
      const tuple = ent[sideKey];
      if (!tuple) continue;
      if (Array.isArray(tuple)) {
        if (tuple[4] === SPLASH_MOVE_ID) return true;
      } else if (typeof tuple === 'object') {
        if (tuple.effective_move_id_if_moved === SPLASH_MOVE_ID) return true;
      }
    }
  }
  return false;
}

module.exports = {
  signatureMentionsSplash,
  SPLASH_MOVE_ID,
};
