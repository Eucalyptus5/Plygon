'use strict';

const CLASSIFICATIONS = Object.freeze({
  PASS: 'PASS',
  SHOWDOWN_REJECTED: 'showdown_rejected',
  HARNESS_ERROR: 'harness_error',
  NEW_BUG: 'new_bug',
  SUPPRESSED_HIGH: 'suppressed_high',
  NEW_BUG_LOW_CONF: 'new_bug_low_conf',
  ENGINE_ERROR: 'engine_error',
  ENGINE_CRASH: 'engine_crash',
  ENGINE_OUTPUT_MALFORMED: 'engine_output_malformed',
  HIGH: 'high_confidence',
  LOW: 'low_confidence',
  NONE: 'none',
});

const SIGNATURE_CATEGORIES = Object.freeze([
  // team.* is per-mon, paired by build_slot; the two HP direction tails never collapse into one.
  'team.current_hp_decreased',
  'team.current_hp_increased',
  'team.status',
  'team.is_fainted',
  // Listed here only so the unknown-category fail-closed branch does not fire on it.
  'team.unpaired_count',
  'team.item_id',
  'team.ability_id',
  // Seven explicit boost tails, one diff per stat; never an active.boosts.* aggregate.
  'active.boosts.atk',
  'active.boosts.def',
  'active.boosts.spa',
  'active.boosts.spd',
  'active.boosts.spe',
  'active.boosts.accuracy',
  'active.boosts.evasion',
  // Sub-volatiles are tracked individually; never a single volatile_changed.
  'active.substitute_hp',
  'active.confusion_turns',
  'active.taunt_turns',
  'active.encore_turns',
  'active.is_terastallized',
  'active.types',
  'active.effective_ability_id',
  'active.effective_item_id',
  'active.effective_species_id',
  // field is emitted at the snapshot root, with no side segment.
  'field.weather',
  'field.terrain',
  // side_conditions keeps the engine's key names.
  'side_conditions.stealth_rock',
  'side_conditions.spikes',
  'side_conditions.toxic_spikes',
  'side_conditions.sticky_web',
  'side_conditions.reflect_turns',
  'side_conditions.light_screen_turns',
  'side_conditions.aurora_veil_turns',
]);

const SIGNATURE_CATEGORIES_SET = new Set(SIGNATURE_CATEGORIES);

// Excluded on purpose: Transform/Illusion trips would be spurious, and unpaired_count is a harness axis.
const REQUIRED_CATEGORIES = Object.freeze([
  'team.current_hp_decreased',
  'team.current_hp_increased',
  'team.status',
  'team.is_fainted',
  'team.item_id',
  'team.ability_id',
  'active.boosts.atk', 'active.boosts.def', 'active.boosts.spa',
  'active.boosts.spd', 'active.boosts.spe', 'active.boosts.accuracy',
  'active.boosts.evasion',
  'side_conditions.stealth_rock', 'side_conditions.spikes',
  'side_conditions.toxic_spikes', 'side_conditions.sticky_web',
  'side_conditions.reflect_turns', 'side_conditions.light_screen_turns',
  'side_conditions.aurora_veil_turns',
  'field.weather', 'field.terrain',
  'active.is_terastallized', 'active.types',
  'active.effective_ability_id', 'active.effective_item_id',
]);

const ORACLE_SCOPE_ALLOWLIST = Object.freeze(new Set([
  'pp_changed',
  'active.substitute_hp',
  'active.confusion_turns',
  'active.taunt_turns',
  'active.encore_turns',
]));

const ACTION_STRUGGLE = 255;

function path_to_category(path, showdown, engine) {
  if (typeof path !== 'string' || path.length === 0) return path;

  // Both prefixes collapse to one category: the all_rolls branch shares the snapshot vocabulary.
  let tail = path;
  const m = tail.match(/^turns\[\d+\]\.(?:state_after|roll\[\d+\])\.(.*)$/);
  if (m) tail = m[1];

  if (tail.startsWith('p1.')) tail = tail.slice(3);
  else if (tail.startsWith('p2.')) tail = tail.slice(3);

  tail = tail.replace(/^team\[\d+\]\./, 'team.');

  if (tail === 'team.current_hp') {
    if (showdown === undefined && engine === undefined) {
      // Argless is the categories_checked case, which needs both direction tails.
      return ['team.current_hp_decreased', 'team.current_hp_increased'];
    }
    if (typeof engine === 'number' && typeof showdown === 'number') {
      if (engine < showdown) return 'team.current_hp_decreased';
      if (engine > showdown) return 'team.current_hp_increased';
      // A tie is arbitrarily booked as decreased.
      return 'team.current_hp_decreased';
    }
    return 'team.current_hp_decreased';
  }

  // An unknown tail is returned as-is so the caller can route it fail-closed.
  if (SIGNATURE_CATEGORIES_SET.has(tail)) return tail;
  return tail;
}

module.exports = {
  CLASSIFICATIONS,
  SIGNATURE_CATEGORIES,
  SIGNATURE_CATEGORIES_SET,
  REQUIRED_CATEGORIES,
  ORACLE_SCOPE_ALLOWLIST,
  ACTION_STRUGGLE,
  path_to_category,
};
