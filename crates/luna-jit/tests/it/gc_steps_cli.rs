//! The collector driven one basic step at a time: each script in
//! `tests/gc_steps/<dialect>/` prints the count after each step and when
//! its finalizers ran (or after a number of rounds), and the luna command
//! must print what PUC printed (the recorded `.out`). PUC 5.2 and 5.3 seed
//! their string hashes from the time and their addresses; their recordings
//! are from a PUC whose seed is fixed at 1, the seed luna takes from
//! `LUNA_HASH_SEED`, run without address randomisation. Both run as `lua`, from
//! the scripts' directory: the program's name and the script's path are
//! strings the collector counts. A script whose output depends on the C
//! library has a recording of its own for macOS (`<name>.macos.out`, from
//! PUC built with `make macosx` and the same fixed seed). The scripts in
//! `tests/lua_c/<dialect>/`
//! count what lua.c itself makes, the same way.

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

/// The name of the recordings made on this platform's PUC.
const PLATFORM: &str = if cfg!(target_vendor = "apple") {
    "macos"
} else {
    "linux"
};

/// The scripts of `dialect`, each run in the modes `jit` lists.
fn check(dialect: &str, jit: &[bool]) {
    check_in("tests/gc_steps", dialect, jit);
}

/// [`check`] of the scripts under `base`.
fn check_in(base: &str, dialect: &str, jit: &[bool]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(base);
    let mut scripts: Vec<_> = std::fs::read_dir(dir.join(dialect))
        .expect("the dialect's scripts")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".lua"))
        .collect();
    scripts.sort();
    assert!(!scripts.is_empty(), "{dialect}: no script");
    for name in scripts {
        // a recording of PUC on this platform where its C library's BUFSIZ
        // (the room 5.1's and 5.2's buffers have before they allocate)
        // makes it differ from glibc's
        let own = dir
            .join(dialect)
            .join(name.replace(".lua", &format!(".{PLATFORM}.out")));
        let path = if own.exists() {
            own
        } else {
            dir.join(dialect).join(name.replace(".lua", ".out"))
        };
        let want = std::fs::read_to_string(path).expect("the recorded output");
        for &j in jit {
            let got = run(&dir, dialect, &format!("{dialect}/{name}"), j);
            assert_eq!(got, want, "{dialect}/{name} jit={j}");
        }
    }
}

#[test]
fn steps_5_1() {
    check("5.1", &[false, true]);
}

#[test]
fn steps_5_2() {
    check("5.2", &[false, true]);
}

/// 5.3 and 5.4 make some names of luna's (`"__metatable"`, `"__pairs"`)
/// where a function first uses them and again after a sweep freed them.
#[test]
fn steps_5_3() {
    check("5.3", &[false]);
}

#[test]
fn steps_5_4() {
    check("5.4", &[false]);
}

/// What lua.c's message handler makes counts as PUC's does: an error
/// object that is no string is described with `lua_pushfstring`.
#[test]
fn lua_c_message_handler() {
    for d in ["5.3", "5.4", "5.5"] {
        check_in("tests/lua_c", d, &[false]);
    }
}
