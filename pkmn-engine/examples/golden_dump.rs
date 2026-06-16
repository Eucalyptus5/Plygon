use pkmn_engine::state::*;

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

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn battle_digest(template: &BattleState, teams: &TeamData, seed: u32) -> (u64, u32) {
    let mut state = *template;
    let mut rng = deterministic_rng(seed);
    let mut h = FNV_OFFSET;
    let mut turns = 0u32;
    for _ in 0..200 {
        if state.is_game_over() { break; }
        turns += 1;
        let a1 = legal_actions(&state, 0);
        let a2 = legal_actions(&state, 1);
        let act1 = if a1.count > 0 { a1.actions[rng(a1.count as u32) as usize] } else { 0 };
        let act2 = if a2.count > 0 { a2.actions[rng(a2.count as u32) as usize] } else { 0 };
        match state.phase {
            PHASE_ACTIONS => execute_turn(&mut state, teams, act1, act2, &mut rng),
            PHASE_SWITCH_P1 | PHASE_SWITCH_P2 | PHASE_SWITCH_BOTH => {
                execute_switch_turn(&mut state, teams, act1, act2, &mut rng);
            }
            _ => break,
        }
        h = fnv(h, &(act1 as u16).to_le_bytes());
        h = fnv(h, &(act2 as u16).to_le_bytes());
        h = fnv(h, &[state.phase]);
        for side in 0..2 {
            let hp: u32 = state.sides[side].team.iter().map(|m| m.current_hp as u32).sum();
            h = fnv(h, &hp.to_le_bytes());
        }
    }
    let js = serde_json::to_string(&state).unwrap();
    (fnv(h, js.as_bytes()), turns)
}

fn main() {
    let mut template = BattleState::default();
    let mons_p1 = [
        make_mon(25, 300, [150, 100, 150, 100, 120], [85, 89, 14, 150], [24, 16, 32, 64]),
        make_mon(6, 280, [130, 110, 100, 130, 100], [53, 58, 89, 150], [24, 16, 16, 64]),
        make_mon(130, 350, [180, 100, 80, 120, 80], [89, 34, 349, 150], [16, 24, 32, 64]),
        make_mon(143, 500, [200, 80, 200, 80, 30], [34, 89, 150, 150], [24, 16, 64, 64]),
        make_mon(94, 260, [110, 80, 170, 100, 130], [85, 58, 53, 150], [24, 16, 24, 64]),
        make_mon(248, 350, [170, 130, 120, 130, 70], [89, 34, 282, 150], [16, 24, 32, 64]),
    ];
    let mons_p2 = [
        make_mon(445, 350, [170, 120, 130, 100, 110], [89, 53, 349, 150], [16, 24, 32, 64]),
        make_mon(242, 600, [30, 80, 200, 200, 60], [85, 58, 150, 150], [24, 16, 64, 64]),
        make_mon(598, 300, [140, 120, 80, 170, 20], [89, 282, 446, 150], [16, 32, 32, 64]),
        make_mon(887, 280, [140, 80, 130, 80, 170], [85, 53, 89, 150], [24, 24, 16, 64]),
        make_mon(823, 350, [130, 120, 80, 120, 80], [89, 34, 369, 150], [16, 24, 24, 64]),
        make_mon(984, 350, [160, 120, 100, 100, 110], [89, 370, 282, 150], [16, 8, 32, 64]),
    ];
    for i in 0..6 {
        template.sides[0].team[i] = mons_p1[i];
        template.sides[1].team[i] = mons_p2[i];
    }
    let teams = TeamData::default();

    const NUM_BATTLES: u32 = 20_000;
    let full = std::env::var("GOLDEN_FULL").is_ok();
    let mut master = FNV_OFFSET;
    let mut total_turns = 0u64;
    for i in 0..NUM_BATTLES {
        let seed = i.wrapping_mul(2654435761);
        let (d, turns) = battle_digest(&template, &teams, seed);
        master = fnv(master, &d.to_le_bytes());
        total_turns += turns as u64;
        if full {
            println!("{} {:016x} {}", seed, d, turns);
        }
    }
    println!("GOLDEN battles={} total_turns={} digest={:016x}", NUM_BATTLES, total_turns, master);
}
