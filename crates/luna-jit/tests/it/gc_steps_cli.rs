//! The collector driven one basic step at a time: each script in
//! `tests/gc_steps/<dialect>/` prints the count after each step and when
//! its finalizers ran, and the luna command must print what PUC printed
//! (the recorded `.out`). PUC 5.2 seeds its string hashes from the time
//! and its addresses; its recordings are from a PUC whose seed is fixed at
//! 1, the seed luna takes from `LUNA_HASH_SEED`. Both run as `lua`, from
//! the scripts' directory: the program's name and the script's path are
//! strings the collector counts.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

fn run(dir: &Path, dialect: &str, script: &str, jit: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_luna"));
    cmd.arg0("lua")
        .current_dir(dir)
        .arg(format!("--lua={dialect}"));
    if !jit {
        cmd.arg("--no-jit");
    }
    let out = cmd
        .arg(script)
        .env("TZ", "UTC")
        .env("LUNA_HASH_SEED", "1")
        .output()
        .expect("run luna");
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

/// The scripts of `dialect`, each run in the modes `jit` lists.
fn check(dialect: &str, jit: &[bool]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gc_steps");
    let mut scripts: Vec<_> = std::fs::read_dir(dir.join(dialect))
        .expect("the dialect's scripts")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".lua"))
        .collect();
    scripts.sort();
    assert!(!scripts.is_empty(), "{dialect}: no script");
    for name in scripts {
        let want = std::fs::read_to_string(dir.join(dialect).join(name.replace(".lua", ".out")))
            .expect("the recorded output");
        for &j in jit {
            let got = run(&dir, dialect, &format!("{dialect}/{name}"), j);
            assert_eq!(got, want, "{dialect}/{name} jit={j}");
        }
    }
}

#[test]
fn steps_5_1() {
    check("5.1", &[false]);
}

#[test]
fn steps_5_2() {
    check("5.2", &[false, true]);
}
