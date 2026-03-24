use criterion::{black_box, criterion_group, criterion_main, Criterion};

use pkmn_engine::data::base_stats::species;
use pkmn_engine::data::moves::{
    electro_ball_bp, flail_bp, gyro_ball_bp, heavy_slam_bp, move_data, move_meta, punishment_bp, stored_power_bp, weight_based_bp
};
use pkmn_engine::data::types::{dual_type_effectiveness, type_effectiveness, Type};
use pkmn_engine::data::{GEN_MOVES, GEN_SPECIES};

use pkmn_engine::state::*;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash};
use pkmn_engine::state::move_exec::execute_move;
use pkmn_engine::state::end_of_turn::end_of_turn;
use pkmn_engine::state::switch::perform_switch;
use pkmn_engine::state::calc::calc_damage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn deterministic_rng() -> impl FnMut(u32) -> u32 {
    let mut seed: u32 = 12345;
    move |max: u32| -> u32 {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        if max == 0 { return 0; }
        (seed >> 16) % max
    }
}

fn make_mon(species_id: u16, hp: u16, stats: [u16; 5], moves: [u16; 4], pp: [u8; 4]) -> MonSlot {
    MonSlot {
        species_id,
        current_hp: hp,
        max_hp: hp,
        stats,
        moves,
        pp,
        ..Default::default()
    }
}

fn setup_vanilla() -> (BattleState, ZobristKeys) {
    let keys = ZobristKeys::new(42);
    let mut state = BattleState::default();

    let mon_a = make_mon(25, 300, [150, 100, 150, 100, 100], [1, 2, 3, 4], [24, 24, 24, 24]);
    let mon_b = make_mon(6, 200, [100, 80, 100, 80, 80], [1, 2, 3, 4], [24, 24, 24, 24]);

    for side in 0..2 {
        state.sides[side].team[0] = mon_a;
        for i in 1..6 {
            state.sides[side].team[i] = mon_b;
        }
    }

    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

fn setup_with_abilities() -> (BattleState, ZobristKeys) {
    let (mut state, keys) = setup_vanilla();
    state.sides[0].team[0].ability_id = 74;  // Pure Power
    state.sides[1].team[0].ability_id = 22;  // Intimidate
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

fn setup_with_items() -> (BattleState, ZobristKeys) {
    let (mut state, keys) = setup_vanilla();
    state.sides[0].team[0].item_id = 68;   // Choice Band
    state.sides[1].team[0].item_id = 249;  // Life Orb
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

fn setup_with_weather() -> (BattleState, ZobristKeys) {
    let (mut state, keys) = setup_vanilla();
    state.field.weather = WEATHER_SUN;
    state.field.weather_turns = 5;
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

fn setup_with_status() -> (BattleState, ZobristKeys) {
    let (mut state, keys) = setup_vanilla();
    state.sides[0].team[0].status = STATUS_BURN;
    state.sides[0].team[0].status_counter = 1;
    state.sides[1].team[0].status = STATUS_BAD_POISON;
    state.sides[1].team[0].status_counter = 1;
    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

fn setup_complex() -> (BattleState, ZobristKeys) {
    let (mut state, keys) = setup_vanilla();
    // Side 0: Pure Power + Life Orb + Burn + boosted
    state.sides[0].team[0].ability_id = 74;  // Pure Power
    state.sides[0].team[0].item_id = 249;    // Life Orb
    state.sides[0].team[0].status = STATUS_BURN;
    state.sides[0].active.boosts[0] = 2;     // +2 Atk

    // Side 1: Multiscale + Assault Vest
    state.sides[1].team[0].ability_id = 136;  // Multiscale
    state.sides[1].team[0].item_id = 581;     // Assault Vest

    // Rain weather
    state.field.weather = WEATHER_RAIN;
    state.field.weather_turns = 5;

    // Stealth Rock on side 0
    state.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;

    state.zobrist = compute_full_hash(&state, &keys);
    (state, keys)
}

/// Run an MCTS-style rollout: legal_actions -> execute_turn, handling switch phases.
fn run_rollout(state: &mut BattleState, keys: &ZobristKeys, max_turns: u32, rng: &mut impl FnMut(u32) -> u32) {
    for _ in 0..max_turns {
        if state.is_game_over() { break; }

        let a1 = legal_actions(state, 0);
        let a2 = legal_actions(state, 1);
        let act1 = if a1.count > 0 { a1.actions[0] } else { 0 };
        let act2 = if a2.count > 0 { a2.actions[0] } else { 0 };

        match state.phase {
            PHASE_ACTIONS => execute_turn(state, keys, act1, act2, rng),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                execute_switch_turn(state, keys, act1, act2, rng);
            }
            _ => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Existing benchmarks (unchanged)
// ---------------------------------------------------------------------------

fn bench_type_lookups(c: &mut Criterion) {
    let mut group = c.benchmark_group("Type Effectiveness");

    group.bench_function("Single Type Lookup", |b| {
        b.iter(|| {
            // Benchmark a neutral lookup
            let eff = type_effectiveness(black_box(Type::Normal), black_box(Type::Water));
            black_box(eff);
        });
    });

    group.bench_function("Dual Type Lookup (Neutral)", |b| {
        b.iter(|| {
            let eff = dual_type_effectiveness(
                black_box(Type::Fire),
                black_box(Type::Grass),
                black_box(Type::Water),
            );
            black_box(eff);
        });
    });

    group.bench_function("Dual Type Lookup (Mono Defender)", |b| {
        b.iter(|| {
            // Simulates def1 == def2 (Mono-type logic path)
            let eff = dual_type_effectiveness(
                black_box(Type::Fire),
                black_box(Type::Grass),
                black_box(Type::Grass),
            );
            black_box(eff);
        });
    });

    group.finish();
}

fn bench_data_lookups(c: &mut Criterion) {
    let mut group = c.benchmark_group("Data Array Lookups");

    let num_species = GEN_SPECIES.len();
    group.bench_function("Species Lookup", |b| {
        b.iter(|| {
            // Pick a pseudo-random index
            let i = black_box(150 % num_species);
            let s = species(i);
            black_box(s);
        });
    });

    let num_moves = GEN_MOVES.len();
    group.bench_function("Move Data Lookup", |b| {
        b.iter(|| {
            let i = black_box(33 % num_moves);
            let m = move_data(i);
            black_box(m);
        });
    });

    group.bench_function("Move Meta Lookup", |b| {
        b.iter(|| {
            let i = black_box(33 % num_moves);
            let m = move_meta(i);
            black_box(m);
        });
    });

    group.finish();
}

fn bench_variable_bp(c: &mut Criterion) {
    let mut group = c.benchmark_group("Variable BP Resolvers");

    group.bench_function("Weight Based BP", |b| {
        b.iter(|| {
            let bp = weight_based_bp(black_box(1500)); // 150.0 kg
            black_box(bp);
        });
    });

    group.bench_function("Heavy Slam BP", |b| {
        b.iter(|| {
            let bp = heavy_slam_bp(black_box(1000), black_box(200));
            black_box(bp);
        });
    });

    group.bench_function("Gyro Ball BP", |b| {
        b.iter(|| {
            let bp = gyro_ball_bp(black_box(50), black_box(150));
            black_box(bp);
        });
    });

    group.bench_function("Flail BP", |b| {
        b.iter(|| {
            let bp = flail_bp(black_box(10), black_box(100));
            black_box(bp);
        });
    });

    group.bench_function("Electro Ball BP", |b| {
        b.iter(|| {
            let bp = electro_ball_bp(black_box(200), black_box(80));
            black_box(bp);
        });
    });

    group.bench_function("Stored Power BP", |b| {
        b.iter(|| {
            let bp = stored_power_bp(black_box(6));
            black_box(bp);
        });
    });

    group.bench_function("Punishment BP", |b| {
        b.iter(|| {
            let bp = punishment_bp(black_box(6));
            black_box(bp);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: MCTS simulation throughput
// ---------------------------------------------------------------------------

fn bench_mcts_simulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("MCTS Simulation");

    let (template_v, keys_v) = setup_vanilla();
    let (template_c, keys_c) = setup_complex();

    group.bench_function("Rollout (Vanilla, 50 turns)", |b| {
        b.iter(|| {
            let mut state = template_v;
            let mut rng = deterministic_rng();
            run_rollout(&mut state, &keys_v, 50, &mut rng);
            black_box(state);
        });
    });

    group.bench_function("Rollout (Complex, 50 turns)", |b| {
        b.iter(|| {
            let mut state = template_c;
            let mut rng = deterministic_rng();
            run_rollout(&mut state, &keys_c, 50, &mut rng);
            black_box(state);
        });
    });

    group.bench_function("Rollout (Vanilla, 200 turns)", |b| {
        b.iter(|| {
            let mut state = template_v;
            let mut rng = deterministic_rng();
            run_rollout(&mut state, &keys_v, 200, &mut rng);
            black_box(state);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: execute_turn
// ---------------------------------------------------------------------------

fn bench_execute_turn(c: &mut Criterion) {
    let mut group = c.benchmark_group("Execute Turn");

    let setups: Vec<(&str, BattleState, ZobristKeys)> = vec![
        { let (s, k) = setup_vanilla(); ("Vanilla", s, k) },
        { let (s, k) = setup_with_abilities(); ("With Abilities", s, k) },
        { let (s, k) = setup_with_items(); ("With Items", s, k) },
        { let (s, k) = setup_with_weather(); ("With Weather", s, k) },
        { let (s, k) = setup_with_status(); ("With Status", s, k) },
        { let (s, k) = setup_complex(); ("Complex", s, k) },
    ];

    for (name, template, keys) in &setups {
        group.bench_function(*name, |b| {
            b.iter(|| {
                let mut state = *template;
                let mut rng = deterministic_rng();
                execute_turn(&mut state, keys, 0, 0, &mut rng);
                black_box(state);
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: legal_actions
// ---------------------------------------------------------------------------

fn bench_legal_actions(c: &mut Criterion) {
    let mut group = c.benchmark_group("Legal Actions");

    let (state_full, _) = setup_vanilla();

    group.bench_function("Full Team", |b| {
        b.iter(|| {
            let actions = legal_actions(black_box(&state_full), black_box(0));
            black_box(actions);
        });
    });

    // Limited: only 2 moves with PP, 2 alive backmons
    let mut state_limited = state_full;
    state_limited.sides[0].team[0].moves[2] = 0;
    state_limited.sides[0].team[0].moves[3] = 0;
    state_limited.sides[0].team[0].pp[2] = 0;
    state_limited.sides[0].team[0].pp[3] = 0;
    for i in 3..6 {
        state_limited.sides[0].team[i].current_hp = 0;
    }

    group.bench_function("Limited (2 moves, 2 backmons)", |b| {
        b.iter(|| {
            let actions = legal_actions(black_box(&state_limited), black_box(0));
            black_box(actions);
        });
    });

    // Switch phase
    let mut state_switch = state_full;
    state_switch.phase = PHASE_SWITCH_P1;

    group.bench_function("Switch Phase", |b| {
        b.iter(|| {
            let actions = legal_actions(black_box(&state_switch), black_box(0));
            black_box(actions);
        });
    });

    // Trapped
    let mut state_trapped = state_full;
    state_trapped.sides[0].active.volatile_flags |= VOL_TRAPPED;

    group.bench_function("Trapped", |b| {
        b.iter(|| {
            let actions = legal_actions(black_box(&state_trapped), black_box(0));
            black_box(actions);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: calc_damage
// ---------------------------------------------------------------------------

fn bench_calc_damage(c: &mut Criterion) {
    let mut group = c.benchmark_group("Calc Damage");

    let (state_v, _) = setup_vanilla();
    let (state_ab, _) = setup_with_abilities();
    let (state_it, _) = setup_with_items();
    let (state_w, _) = setup_with_weather();
    let (state_cx, _) = setup_complex();

    // Vanilla physical (move 1 = Pound)
    group.bench_function("Vanilla Physical", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_v), black_box(0), black_box(1), black_box(0), &mut rng);
            black_box(result);
        });
    });

    // Vanilla special (move 2)
    group.bench_function("Vanilla Special", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_v), black_box(0), black_box(2), black_box(0), &mut rng);
            black_box(result);
        });
    });

    // With abilities (Pure Power attacker)
    group.bench_function("With Abilities", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_ab), black_box(0), black_box(1), black_box(0), &mut rng);
            black_box(result);
        });
    });

    // With items (Choice Band attacker)
    group.bench_function("With Items", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_it), black_box(0), black_box(1), black_box(0), &mut rng);
            black_box(result);
        });
    });

    // Weather boosted
    group.bench_function("Weather Boosted", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_w), black_box(0), black_box(1), black_box(0), &mut rng);
            black_box(result);
        });
    });

    // Burned attacker (physical)
    let mut state_burn = state_v;
    state_burn.sides[0].team[0].status = STATUS_BURN;
    group.bench_function("Burned Attacker", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_burn), black_box(0), black_box(1), black_box(0), &mut rng);
            black_box(result);
        });
    });

    // Complex (all modifiers)
    group.bench_function("Complex", |b| {
        b.iter(|| {
            let mut rng = deterministic_rng();
            let result = calc_damage(black_box(&state_cx), black_box(0), black_box(1), black_box(0), &mut rng);
            black_box(result);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: execute_move (full 25-hook pipeline)
// ---------------------------------------------------------------------------

fn bench_execute_move(c: &mut Criterion) {
    let mut group = c.benchmark_group("Execute Move");

    let (template_v, keys_v) = setup_vanilla();
    let (template_cx, keys_cx) = setup_complex();

    // Vanilla physical
    group.bench_function("Vanilla Physical", |b| {
        b.iter(|| {
            let mut state = template_v;
            let mut rng = deterministic_rng();
            execute_move(&mut state, &keys_v, 0, 1, 0, &mut rng);
            black_box(state);
        });
    });

    // Status move (move slot 3 = move ID 4, likely a status move; if not, still exercises the pipeline)
    group.bench_function("Status Move", |b| {
        b.iter(|| {
            let mut state = template_v;
            let mut rng = deterministic_rng();
            execute_move(&mut state, &keys_v, 0, 4, 3, &mut rng);
            black_box(state);
        });
    });

    // Complex
    group.bench_function("Complex", |b| {
        b.iter(|| {
            let mut state = template_cx;
            let mut rng = deterministic_rng();
            execute_move(&mut state, &keys_cx, 0, 1, 0, &mut rng);
            black_box(state);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: end_of_turn and perform_switch
// ---------------------------------------------------------------------------

fn bench_eot_and_switch(c: &mut Criterion) {
    let mut group = c.benchmark_group("EOT and Switch");

    let (template_v, keys_v) = setup_vanilla();

    // End of turn: vanilla (fast path, no weather/status/hazards)
    group.bench_function("end_of_turn (Vanilla)", |b| {
        b.iter(|| {
            let mut state = template_v;
            end_of_turn(&mut state, &keys_v);
            black_box(state);
        });
    });

    // End of turn: weather + status
    let (template_ws, keys_ws) = setup_with_status();
    let mut template_ws = template_ws;
    template_ws.field.weather = WEATHER_SAND;
    template_ws.field.weather_turns = 5;

    group.bench_function("end_of_turn (Weather + Status)", |b| {
        b.iter(|| {
            let mut state = template_ws;
            end_of_turn(&mut state, &keys_ws);
            black_box(state);
        });
    });

    // Switch: vanilla (no hazards)
    group.bench_function("perform_switch (Vanilla)", |b| {
        b.iter(|| {
            let mut state = template_v;
            perform_switch(&mut state, &keys_v, 0, 1);
            black_box(state);
        });
    });

    // Switch: with hazards (Stealth Rock + 2 Spikes)
    let mut template_hz = template_v;
    template_hz.sides[0].side_conditions.hazard_flags |= HAZARD_STEALTH_ROCK;
    template_hz.sides[0].side_conditions.spikes = 2;

    group.bench_function("perform_switch (With Hazards)", |b| {
        b.iter(|| {
            let mut state = template_hz;
            perform_switch(&mut state, &keys_v, 0, 1);
            black_box(state);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// New benchmarks: Zobrist hashing
// ---------------------------------------------------------------------------

fn bench_zobrist(c: &mut Criterion) {
    let mut group = c.benchmark_group("Zobrist");

    let (state, keys) = setup_vanilla();

    group.bench_function("compute_full_hash", |b| {
        b.iter(|| {
            let hash = compute_full_hash(black_box(&state), black_box(&keys));
            black_box(hash);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_type_lookups,
    bench_data_lookups,
    bench_variable_bp,
    bench_mcts_simulation,
    bench_execute_turn,
    bench_legal_actions,
    bench_calc_damage,
    bench_execute_move,
    bench_eot_and_switch,
    bench_zobrist,
);
criterion_main!(benches);
