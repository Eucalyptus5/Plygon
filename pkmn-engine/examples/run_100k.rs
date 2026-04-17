use std::time::Instant;

use pkmn_engine::state::*;
use pkmn_engine::state::zobrist::{ZobristKeys, compute_full_hash};

fn deterministic_rng(seed: u32) -> impl FnMut(u32) -> u32 {
    let mut s: u32 = seed;
    move |max: u32| -> u32 {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        if max == 0 { return 0; }
        (s >> 16) % max
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
        level: 100,
        ..Default::default()
    }
}

fn run_battle(template: &BattleState, keys: &ZobristKeys, teams: &TeamData, seed: u32) -> u32 {
    let mut state = *template;
    let mut rng = deterministic_rng(seed);
    let mut turns = 0u32;

    for _ in 0..200 {
        if state.is_game_over() { break; }
        turns += 1;

        let a1 = legal_actions(&state, 0);
        let a2 = legal_actions(&state, 1);

        // Pick a pseudo-random legal action for each side
        let act1 = if a1.count > 0 {
            let idx = rng(a1.count as u32) as usize;
            a1.actions[idx]
        } else { 0 };
        let act2 = if a2.count > 0 {
            let idx = rng(a2.count as u32) as usize;
            a2.actions[idx]
        } else { 0 };

        match state.phase {
            PHASE_ACTIONS => execute_turn(&mut state, keys, teams, act1, act2, &mut rng),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                execute_switch_turn(&mut state, keys, teams, act1, act2, &mut rng);
            }
            _ => break,
        }
    }
    turns
}

fn main() {
    let keys = ZobristKeys::new(42);
    let mut template = BattleState::default();

    // Diverse team: different species, abilities, items, moves
    let mons_p1 = [
        make_mon(25, 300, [150, 100, 150, 100, 120], [85, 89, 14, 150], [24, 16, 32, 64]),   // Pikachu: Tbolt/EQ/SD/Splash
        make_mon(6, 280, [130, 110, 100, 130, 100], [53, 58, 89, 150], [24, 16, 16, 64]),     // Charizard: Flamethrower/IceBeam/EQ/Splash
        make_mon(130, 350, [180, 100, 80, 120, 80], [89, 34, 349, 150], [16, 24, 32, 64]),    // Gyarados: EQ/BodySlam/DDance/Splash
        make_mon(143, 500, [200, 80, 200, 80, 30], [34, 89, 150, 150], [24, 16, 64, 64]),     // Snorlax: BodySlam/EQ/Splash/Splash
        make_mon(94, 260, [110, 80, 170, 100, 130], [85, 58, 53, 150], [24, 16, 24, 64]),     // Gengar: Tbolt/IceBeam/Flamethrower/Splash
        make_mon(248, 350, [170, 130, 120, 130, 70], [89, 34, 282, 150], [16, 24, 32, 64]),   // Tyranitar: EQ/BodySlam/KnockOff/Splash
    ];
    let mons_p2 = [
        make_mon(445, 350, [170, 120, 130, 100, 110], [89, 53, 349, 150], [16, 24, 32, 64]), // Garchomp
        make_mon(242, 600, [30, 80, 200, 200, 60], [85, 58, 150, 150], [24, 16, 64, 64]),    // Blissey
        make_mon(598, 300, [140, 120, 80, 170, 20], [89, 282, 446, 150], [16, 32, 32, 64]),  // Ferrothorn
        make_mon(887, 280, [140, 80, 130, 80, 170], [85, 53, 89, 150], [24, 24, 16, 64]),    // Dragapult
        make_mon(823, 350, [130, 120, 80, 120, 80], [89, 34, 369, 150], [16, 24, 24, 64]),   // Corviknight
        make_mon(984, 350, [160, 120, 100, 100, 110], [89, 370, 282, 150], [16, 8, 32, 64]), // Great Tusk
    ];

    for i in 0..6 {
        template.sides[0].team[i] = mons_p1[i];
        template.sides[1].team[i] = mons_p2[i];
    }
    template.zobrist = compute_full_hash(&template, &keys);

    let teams = TeamData::default();

    const NUM_BATTLES: u32 = 100_000;

    println!("Running {} battles...", NUM_BATTLES);
    let start = Instant::now();

    let mut total_turns = 0u64;

    for i in 0..NUM_BATTLES {
        let turns = run_battle(&template, &keys, &teams, i.wrapping_mul(2654435761));
        total_turns += turns as u64;
    }

    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let battles_per_sec = NUM_BATTLES as f64 / secs;
    let avg_turns = total_turns as f64 / NUM_BATTLES as f64;

    println!("===== Results =====");
    println!("Battles:        {}", NUM_BATTLES);
    println!("Total time:     {:.3}s", secs);
    println!("Battles/sec:    {:.0}", battles_per_sec);
    println!("Avg turns/game: {:.1}", avg_turns);
    println!("µs/battle:      {:.2}", secs / NUM_BATTLES as f64 * 1_000_000.0);
}
