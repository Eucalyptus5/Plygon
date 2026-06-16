use pkmn_engine::state::structs::*;

#[test]
fn battlestate_serde_roundtrips() {
    let mut s = BattleState::default();

    s.phase = PHASE_SWITCH_P1;
    s.field.turn = 17;
    s.field.weather = WEATHER_SAND;
    s.field.weather_turns = 5;
    s.field.terrain_turns = 2;
    s.last_move_globally = 442;
    s.pending_actions = [3, 7];

    {
        let side = &mut s.sides[0];
        side.active_index = 1;

        let mon = &mut side.team[1];
        mon.species_id = 887;
        mon.ability_id = 22;
        mon.item_id = 217;
        mon.max_hp = 281;
        mon.current_hp = 96;
        mon.stats = [180, 150, 210, 140, 160];
        mon.moves = [398, 89, 247, 521];
        mon.pp = [16, 8, 4, 24];
        mon.status = STATUS_BURN;
        mon.status_counter = 3;
        mon.tera_type = 12;
        mon.level = 88;
        mon.flags = 0x0102;

        side.active.boosts[ATK] = 2;
        side.active.boosts[SPE] = -1;
        side.active.boosts[DEF] = 1;
        side.active.toxic_counter = 4;
        side.active.confusion_turns = 2;
        side.active.substitute_hp = 70;
        side.active.last_move = 89;
        side.active.turns_active = 6;
        side.active.set_volatile(VOL_SUBSTITUTE);
        side.active.set_volatile(VOL_LEECH_SEED);

        side.side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
        side.side_conditions.spikes = 2;
        side.side_conditions.toxic_spikes = 1;
        side.side_conditions.reflect_turns = 4;

        side.set_last_consumed_berry(217);
    }

    {
        let side = &mut s.sides[1];
        let mon = &mut side.team[0];
        mon.species_id = 130;
        mon.ability_id = 41;
        mon.max_hp = 301;
        mon.current_hp = 301;
        mon.stats = [200, 130, 90, 120, 170];
        mon.moves = [57, 58, 240, 503];
        mon.pp = [8, 8, 16, 16];
        mon.status = STATUS_BAD_POISON;
        mon.status_counter = 2;
        mon.level = 90;

        side.active.boosts[SPA] = 1;
        side.active.toxic_counter = 2;
        side.active.set_volatile(VOL_TORMENT);

        side.side_conditions.tailwind_turns = 3;
        side.side_conditions.wish_hp = 150;
        side.side_conditions.wish_turns = 1;
    }

    let mut t = TeamData::default();
    t.mons[0][0] = MonBuildData { ivs: [31, 0, 31, 31, 31, 31], evs: [252, 0, 0, 4, 252, 0], nature: 7 };
    t.mons[0][1] = MonBuildData { ivs: [31, 31, 31, 31, 31, 31], evs: [0, 252, 252, 4, 0, 0], nature: 3 };
    t.mons[1][0] = MonBuildData { ivs: [31, 31, 0, 31, 31, 31], evs: [248, 8, 0, 0, 252, 0], nature: 12 };
    t.mons[1][3] = MonBuildData { ivs: [0, 31, 31, 31, 31, 31], evs: [4, 252, 0, 252, 0, 0], nature: 19 };
    t.levels[0] = [88, 90, 50, 100, 75, 5];
    t.levels[1] = [90, 100, 80, 60, 5, 50];

    let js = serde_json::to_string(&s).unwrap();
    let s2: BattleState = serde_json::from_str(&js).unwrap();
    assert_eq!(s, s2, "BattleState must round-trip field-for-field");

    let jt = serde_json::to_string(&t).unwrap();
    let t2: TeamData = serde_json::from_str(&jt).unwrap();
    assert_eq!(t, t2, "TeamData must round-trip field-for-field");
}
