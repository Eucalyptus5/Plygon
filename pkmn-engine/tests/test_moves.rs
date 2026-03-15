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
    assert_eq!(weight_based_bp(0), 20);
    assert_eq!(weight_based_bp(99), 20);
    assert_eq!(weight_based_bp(100), 40);
    assert_eq!(weight_based_bp(249), 40);
    assert_eq!(weight_based_bp(250), 60);
    assert_eq!(weight_based_bp(499), 60);
    assert_eq!(weight_based_bp(500), 80);
    assert_eq!(weight_based_bp(999), 80);
    assert_eq!(weight_based_bp(1000), 100);
    assert_eq!(weight_based_bp(1999), 100);
    assert_eq!(weight_based_bp(2000), 120);
    assert_eq!(weight_based_bp(9999), 120);
}

#[test]
fn test_heavy_slam_bp() {
    assert_eq!(heavy_slam_bp(500, 100), 120);
    assert_eq!(heavy_slam_bp(1000, 200), 120);

    assert_eq!(heavy_slam_bp(400, 100), 100);
    assert_eq!(heavy_slam_bp(499, 100), 100);

    assert_eq!(heavy_slam_bp(300, 100), 80);
    assert_eq!(heavy_slam_bp(399, 100), 80);

    assert_eq!(heavy_slam_bp(200, 100), 60);
    assert_eq!(heavy_slam_bp(299, 100), 60);

    assert_eq!(heavy_slam_bp(199, 100), 40);
    assert_eq!(heavy_slam_bp(100, 100), 40);
    assert_eq!(heavy_slam_bp(50, 100), 40);

    assert_eq!(heavy_slam_bp(100, 0), 120);
}

#[test]
fn test_gyro_ball_bp() {
    assert_eq!(gyro_ball_bp(100, 100), 25);
    assert_eq!(gyro_ball_bp(50, 100), 50);
    assert_eq!(gyro_ball_bp(25, 100), 100);
    assert_eq!(gyro_ball_bp(10, 100), 150);
    assert_eq!(gyro_ball_bp(1, 1000), 150);

    assert_eq!(gyro_ball_bp(100, 0), 0);
    assert_eq!(gyro_ball_bp(0, 100), 150);
}

#[test]
fn test_eruption_bp() {
    assert_eq!(eruption_bp(100, 100), 150);
    assert_eq!(eruption_bp(50, 100), 75);
    assert_eq!(eruption_bp(1, 100), 1);
    assert_eq!(eruption_bp(0, 100), 1);

    assert_eq!(eruption_bp(100, 0), 1);
}

#[test]
fn test_flail_bp() {
    assert_eq!(flail_bp(0, 100), 200);
    assert_eq!(flail_bp(2, 100), 200);
    assert_eq!(flail_bp(4, 100), 200);

    assert_eq!(flail_bp(5, 100), 150);
    assert_eq!(flail_bp(10, 100), 150);

    assert_eq!(flail_bp(13, 100), 100);
    assert_eq!(flail_bp(27, 100), 100);

    assert_eq!(flail_bp(28, 100), 80);
    assert_eq!(flail_bp(45, 100), 80);

    assert_eq!(flail_bp(46, 100), 40);
    assert_eq!(flail_bp(68, 100), 40);

    assert_eq!(flail_bp(69, 100), 20);
    assert_eq!(flail_bp(100, 100), 20);

    assert_eq!(flail_bp(100, 0), 200);
}

#[test]
fn test_electro_ball_bp() {
    assert_eq!(electro_ball_bp(10, 100), 40);
    assert_eq!(electro_ball_bp(100, 100), 60);
    assert_eq!(electro_ball_bp(199, 100), 60);
    assert_eq!(electro_ball_bp(200, 100), 80);
    assert_eq!(electro_ball_bp(299, 100), 80);
    assert_eq!(electro_ball_bp(300, 100), 120);
    assert_eq!(electro_ball_bp(399, 100), 120);
    assert_eq!(electro_ball_bp(400, 100), 150);
    assert_eq!(electro_ball_bp(1000, 100), 150);

    assert_eq!(electro_ball_bp(100, 0), 150);
}

#[test]
fn test_stored_power_bp() {
    assert_eq!(stored_power_bp(0), 20);
    assert_eq!(stored_power_bp(1), 40);
    assert_eq!(stored_power_bp(7), 160);
    assert_eq!(stored_power_bp(12), 255);
    assert_eq!(stored_power_bp(42), 255);
}

#[test]
fn test_punishment_bp() {
    assert_eq!(punishment_bp(0), 60);
    assert_eq!(punishment_bp(1), 80);
    assert_eq!(punishment_bp(6), 180);
    assert_eq!(punishment_bp(7), 200);
    assert_eq!(punishment_bp(8), 200);
    assert_eq!(punishment_bp(42), 200);
}
