'use strict';

const TEAM_AXES = ['current_hp', 'status', 'is_fainted', 'item_id', 'ability_id'];

const ACTIVE_BOOST_KEYS = ['atk', 'def', 'spa', 'spd', 'spe', 'accuracy', 'evasion'];
const ACTIVE_SCALAR_AXES = ['substitute_hp', 'confusion_turns', 'taunt_turns', 'encore_turns'];
const ACTIVE_NEW_AXES = [
  'is_terastallized',
  'types',
  'effective_ability_id',
  'effective_item_id',
  'effective_species_id',
];

const SIDE_CONDITION_HAZARDS = ['stealth_rock', 'spikes', 'toxic_spikes', 'sticky_web'];
const SIDE_CONDITION_SCREENS = ['reflect_turns', 'light_screen_turns', 'aurora_veil_turns'];

function enumerateTeamPaths(sidePrefix, sideSnapshot) {
  const paths = [];
  const team = (sideSnapshot && sideSnapshot.team) || [];
  const slotCount = Math.max(team.length, 6);
  for (let i = 0; i < slotCount; i++) {
    for (const axis of TEAM_AXES) {
      paths.push(`${sidePrefix}.team[${i}].${axis}`);
    }
  }
  paths.push(`${sidePrefix}.team.unpaired_count`);
  return paths;
}

function enumerateActivePaths(activePrefix, activeSnapshot) {
  const paths = [];
  for (const k of ACTIVE_BOOST_KEYS) paths.push(`${activePrefix}.boosts.${k}`);
  for (const a of ACTIVE_SCALAR_AXES) paths.push(`${activePrefix}.${a}`);
  for (const a of ACTIVE_NEW_AXES) paths.push(`${activePrefix}.${a}`);
  return paths;
}

function enumerateSideConditionPaths(sideConditionsPrefix, sideConditionsSnapshot) {
  const paths = [];
  for (const h of SIDE_CONDITION_HAZARDS) paths.push(`${sideConditionsPrefix}.${h}`);
  for (const s of SIDE_CONDITION_SCREENS) paths.push(`${sideConditionsPrefix}.${s}`);
  return paths;
}

function enumerateFieldPaths(snapshotPrefix, fieldSnapshot) {
  return [
    `${snapshotPrefix}.field.weather`,
    `${snapshotPrefix}.field.terrain`,
  ];
}

module.exports = {
  enumerateTeamPaths,
  enumerateActivePaths,
  enumerateSideConditionPaths,
  enumerateFieldPaths,
};
