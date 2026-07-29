use poke_mcts::action_features::ACTION_DENSE_DIM;
use poke_mcts::determinize::World;
use poke_mcts::testutil::{duel, mon};
use poke_mcts::train_dump;
use std::path::{Path, PathBuf};

const ACTION_ROW: u64 = 14 * ACTION_DENSE_DIM as u64;
const ROWS: u64 = 2;

fn tmp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("poke_mcts_dfcli_{}_{}", tag, std::process::id()))
}

fn setup_dump(dir: &Path) {
    let (state, teams) = duel(mon(445, 24, [89, 14, 0, 0]), mon(248, 45, [89, 242, 0, 0]));
    std::fs::create_dir_all(dir).unwrap();
    let worlds = vec![World { state, teams, weight: 1.0 }];
    #[cfg(not(feature = "train_value"))]
    train_dump::dump_worlds(dir.to_str().unwrap(), 0, &worlds, 0, 5).unwrap();
    #[cfg(feature = "train_value")]
    train_dump::dump_worlds(dir.to_str().unwrap(), 0, &worlds, 0, 5, 0.5).unwrap();
    std::fs::write(dir.join("outcomes.jsonl"), "{\"game_tag\":0,\"winner_side0\":1.0}\n").unwrap();
}

fn run_cli(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_dump_features"))
        .args(args)
        .output()
        .unwrap()
}

fn meta_of(out: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(out.join("meta.json")).unwrap()).unwrap()
}

fn assert_no_action_dense(out: &Path, what: &str) {
    assert!(!out.join("action_dense.bin").exists(), "{what}: action_dense.bin must not exist");
    let meta = meta_of(out);
    assert!(meta.get("action_dense").is_none(), "{what}: meta must omit action_dense");
    assert!(meta.get("action_dense_dim").is_none(), "{what}: meta must omit action_dense_dim");
}

fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.file_name().unwrap().to_str().unwrap().to_string(), std::fs::read(&p).unwrap())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn defaults_to_full_action_dense() {
    let dir = tmp_dir("default");
    let out = tmp_dir("default_out");
    setup_dump(&dir);
    let r = run_cli(&[dir.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert!(r.status.success(), "default convert failed: {}", String::from_utf8_lossy(&r.stderr));
    assert_eq!(
        std::fs::metadata(out.join("action_dense.bin")).unwrap().len(),
        ROWS * ACTION_ROW * 4,
        "the default mode must write the sidecar"
    );
    let meta = meta_of(&out);
    assert_eq!(meta["action_dense"], true);
    assert_eq!(meta["action_dense_dim"], ACTION_DENSE_DIM as u64);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&out).ok();
}

#[test]
fn no_action_dense_flag_is_an_alias_for_mode_off() {
    let dir = tmp_dir("alias");
    let out_flag = tmp_dir("alias_flag");
    let out_mode = tmp_dir("alias_mode");
    setup_dump(&dir);
    let d = dir.to_str().unwrap();
    let a = run_cli(&[d, "--out", out_flag.to_str().unwrap(), "--no-action-dense"]);
    assert!(a.status.success(), "--no-action-dense failed: {}", String::from_utf8_lossy(&a.stderr));
    let b = run_cli(&[d, "--out", out_mode.to_str().unwrap(), "--action-dense-mode", "off"]);
    assert!(b.status.success(), "mode off failed: {}", String::from_utf8_lossy(&b.stderr));
    assert_no_action_dense(&out_flag, "--no-action-dense");
    assert_no_action_dense(&out_mode, "--action-dense-mode off");
    assert_eq!(
        dir_bytes(&out_flag),
        dir_bytes(&out_mode),
        "--no-action-dense and --action-dense-mode off must produce identical dirs"
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&out_flag).ok();
    std::fs::remove_dir_all(&out_mode).ok();
}

#[test]
fn unknown_action_dense_mode_exits_2() {
    let dir = tmp_dir("badmode");
    let out_ok = tmp_dir("badmode_ok");
    let out_bad = tmp_dir("badmode_bad");
    setup_dump(&dir);
    let d = dir.to_str().unwrap();
    let ok = run_cli(&[d, "--out", out_ok.to_str().unwrap(), "--action-dense-mode", "full"]);
    assert!(ok.status.success(), "a valid mode must be accepted: {}", String::from_utf8_lossy(&ok.stderr));
    let bad = run_cli(&[d, "--out", out_bad.to_str().unwrap(), "--action-dense-mode", "bogus"]);
    assert_eq!(bad.status.code(), Some(2), "an unknown mode must exit 2");
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("--action-dense-mode value"),
        "stderr must name the rejected flag: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&out_ok).ok();
    std::fs::remove_dir_all(&out_bad).ok();
}
