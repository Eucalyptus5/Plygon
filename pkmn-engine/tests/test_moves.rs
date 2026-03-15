use pkmn_engine::data::moves::{
    electro_ball_bp, eruption_bp, flail_bp, gyro_ball_bp, heavy_slam_bp, move_data, move_meta, punishment_bp, stored_power_bp, weight_based_bp, MoveCategory, VarPower,
};

#[test]
fn test_move_data_lookup() {
    let tackle = move_data(33);
    let tackle_meta = move_meta(33);

    assert_eq!(tackle.base_power, 40);
    assert_eq!(tackle.accuracy, 100);
    assert_eq!(tackle.category, MoveCategory::Physical);
    assert_eq!(tackle.var_power, VarPower::None);
    assert_eq!(tackle.priority, 0);

    assert_eq!(tackle_meta.pp, 35);
}

#[test]
fn test_weight_based_bp() {
    assert_eq!(weight_based_bp(0), 20); // 0 kg
    assert_eq!(weight_based_bp(99), 20); // 9.9 kg
    assert_eq!(weight_based_bp(100), 40); // 10.0 kg
    assert_eq!(weight_based_bp(249), 40); // 24.9 kg
    assert_eq!(weight_based_bp(250), 60); // 25.0 kg
    assert_eq!(weight_based_bp(499), 60); // 49.9 kg
    assert_eq!(weight_based_bp(500), 80); // 50.0 kg
    assert_eq!(weight_based_bp(999), 80); // 99.9 kg
    assert_eq!(weight_based_bp(1000), 100); // 100.0 kg
    assert_eq!(weight_based_bp(1999), 100); // 199.9 kg
    assert_eq!(weight_based_bp(2000), 120); // 200.0 kg
    assert_eq!(weight_based_bp(9999), 120); // 999.9 kg
}

#[test]
fn test_heavy_slam_bp() {
    // def_weight * 5 <= atk_weight -> 120
    assert_eq!(heavy_slam_bp(500, 100), 120);
    assert_eq!(heavy_slam_bp(1000, 200), 120);

    // def_weight * 4 <= atk_weight -> 100
    assert_eq!(heavy_slam_bp(400, 100), 100);
    assert_eq!(heavy_slam_bp(499, 100), 100);

    // def_weight * 3 <= atk_weight -> 80
    assert_eq!(heavy_slam_bp(300, 100), 80);
    assert_eq!(heavy_slam_bp(399, 100), 80);

    // def_weight * 2 <= atk_weight -> 60
    assert_eq!(heavy_slam_bp(200, 100), 60);
    assert_eq!(heavy_slam_bp(299, 100), 60);

    // def_weight * 2 > atk_weight -> 40
    assert_eq!(heavy_slam_bp(199, 100), 40);
    assert_eq!(heavy_slam_bp(100, 100), 40);
    assert_eq!(heavy_slam_bp(50, 100), 40); // Target is heavier

    // Edge case: target weight is 0
    assert_eq!(heavy_slam_bp(100, 0), 120);
}

#[test]
fn test_gyro_ball_bp() {
    assert_eq!(gyro_ball_bp(100, 100), 25);
    assert_eq!(gyro_ball_bp(50, 100), 50);
    assert_eq!(gyro_ball_bp(25, 100), 100);
    assert_eq!(gyro_ball_bp(10, 100), 150); // Capped at 150
    assert_eq!(gyro_ball_bp(1, 1000), 150); // Heavily capped

    // Edge cases
    assert_eq!(gyro_ball_bp(100, 0), 0); // Target speed 0
    assert_eq!(gyro_ball_bp(0, 100), 150); // User speed 0 (division by zero guard)
}

#[test]
fn test_eruption_bp() {
    assert_eq!(eruption_bp(100, 100), 150); // Max HP
    assert_eq!(eruption_bp(50, 100), 75); // Half HP
    assert_eq!(eruption_bp(1, 100), 1); // 1 HP
    assert_eq!(eruption_bp(0, 100), 1); // 0 HP -> min 1

    // Edge cases
    assert_eq!(eruption_bp(100, 0), 1); // Max HP 0 guard
}

#[test]
fn test_flail_bp() {
    // Based on ratio = (48 * current_hp) / max_hp
    // 0..=1 -> 200
    assert_eq!(flail_bp(0, 100), 200); // 0
    assert_eq!(flail_bp(2, 100), 200); // 48 * 2 / 100 = 0.96 = 0
    assert_eq!(flail_bp(4, 100), 200); // 48 * 4 / 100 = 1.92 = 1

    // 2..=5 -> 150
    assert_eq!(flail_bp(5, 100), 150); // 48 * 5 / 100 = 2.4 = 2
    assert_eq!(flail_bp(10, 100), 150); // 48 * 10 / 100 = 4.8 = 4

    // 6..=12 -> 100
    assert_eq!(flail_bp(13, 100), 100); // 48 * 13 / 100 = 6.24 = 6
    assert_eq!(flail_bp(27, 100), 100); // 48 * 27 / 100 = 12.96 = 12

    // 13..=21 -> 80
    assert_eq!(flail_bp(28, 100), 80); // 48 * 28 / 100 = 13.44 = 13
    assert_eq!(flail_bp(45, 100), 80); // 48 * 45 / 100 = 21.6 = 21

    // 22..=32 -> 40
    assert_eq!(flail_bp(46, 100), 40); // 48 * 46 / 100 = 22.08 = 22
    assert_eq!(flail_bp(68, 100), 40); // 48 * 68 / 100 = 32.64 = 32

    // 33+ -> 20
    assert_eq!(flail_bp(69, 100), 20); // 48 * 69 / 100 = 33.12 = 33
    assert_eq!(flail_bp(100, 100), 20); // 48 * 100 / 100 = 48 = 48

    // Edge case
    assert_eq!(flail_bp(100, 0), 200); // Max HP 0 guard
}

#[test]
fn test_electro_ball_bp() {
    assert_eq!(electro_ball_bp(10, 100), 40); // ratio = 0
    assert_eq!(electro_ball_bp(100, 100), 60); // ratio = 1
    assert_eq!(electro_ball_bp(199, 100), 60); // ratio = 1 (truncates)
    assert_eq!(electro_ball_bp(200, 100), 80); // ratio = 2
    assert_eq!(electro_ball_bp(299, 100), 80); // ratio = 2 (truncates)
    assert_eq!(electro_ball_bp(300, 100), 120); // ratio = 3
    assert_eq!(electro_ball_bp(399, 100), 120); // ratio = 3 (truncates)
    assert_eq!(electro_ball_bp(400, 100), 150); // ratio = 4 -> 150
    assert_eq!(electro_ball_bp(1000, 100), 150); // ratio = 10 -> 150

    // Edge case
    assert_eq!(electro_ball_bp(100, 0), 150); // Target speed 0 guard
}

#[test]
fn test_stored_power_bp() {
    assert_eq!(stored_power_bp(0), 20);
    assert_eq!(stored_power_bp(1), 40);
    assert_eq!(stored_power_bp(7), 160);
    // Cap at 255 (u8 max)
    // 20 + 20 * 12 = 260 -> 255
    assert_eq!(stored_power_bp(12), 255);
    assert_eq!(stored_power_bp(42), 255); // Absolute max boosts
}

#[test]
fn test_punishment_bp() {
    assert_eq!(punishment_bp(0), 60);
    assert_eq!(punishment_bp(1), 80);
    assert_eq!(punishment_bp(6), 180);
    assert_eq!(punishment_bp(7), 200); // 60 + 20 * 7 = 200
    // Capped at 200
    assert_eq!(punishment_bp(8), 200);
    assert_eq!(punishment_bp(42), 200); // Absolute max boosts
}
