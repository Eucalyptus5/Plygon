use criterion::{black_box, criterion_group, criterion_main, Criterion};

use pkmn_engine::data::base_stats::species;
use pkmn_engine::data::moves::{
    electro_ball_bp, flail_bp, gyro_ball_bp, heavy_slam_bp, move_data, move_meta, punishment_bp, stored_power_bp, weight_based_bp
};
use pkmn_engine::data::types::{dual_type_effectiveness, type_effectiveness, Type};
use pkmn_engine::data::{GEN_MOVES, GEN_SPECIES};

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

criterion_group!(benches, bench_type_lookups, bench_data_lookups, bench_variable_bp);
criterion_main!(benches);
