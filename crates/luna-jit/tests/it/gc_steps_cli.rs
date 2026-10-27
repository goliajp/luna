//! The collector driven one basic step at a time: each script in
//! `tests/gc_steps/<dialect>/` prints the count after each step and when
//! its finalizers ran (or after a number of rounds), and the luna command
//! must print what PUC printed (the recorded `.out`). PUC 5.2 and 5.3 seed
//! their string hashes from the time and their addresses; their recordings
//! are from a PUC whose seed is fixed at 1, the seed luna takes from
//! `LUNA_HASH_SEED`, run without address randomisation. Both run as `lua`, from
//! the scripts' directory: the program's name and the script's path are
//! strings the collector counts. A script whose output depends on the C
//! library or the platform has a recording of its own for macOS and
//! Windows (`<name>.<platform>.out`, from PUC built with `make macosx`, and
//! with MSVC, `cl /O2 /MD`, run from `C:\luna-gc-steps`, with the same
//! fixed seed). The scripts in `tests/lua_c/<dialect>/` count what lua.c
//! itself makes, the same way.

use std::path::Path;
use std::process::Command;

/// The luna command run as `lua`: on Unix by the name it is given; on
/// Windows, where a program's name is what started it, as a copy named
/// `lua.exe` that `cmd` finds on the path. The copy sits where the
/// recordings' PUC sat (`C:\luna-gc-steps`): its directory is in the
/// default package paths (`!`), strings the collector counts.
fn lua_command() -> Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_luna"));
        cmd.arg0("lua");
        cmd
    }
    #[cfg(windows)]
    {
        static COPY: std::sync::Once = std::sync::Once::new();
        let bin = std::path::PathBuf::from(r"C:\luna-gc-steps");
        COPY.call_once(|| {
            std::fs::create_dir_all(&bin).expect("a directory");
            std::fs::copy(env!("CARGO_BIN_EXE_luna"), bin.join("lua.exe")).expect("lua.exe");
        });
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        let mut cmd = Command::new("cmd");
        cmd.env("PATH", std::env::join_paths(paths).expect("a path"));
        cmd.args(["/C", "lua"]);
        cmd
    }
}

fn run(dir: &Path, dialect: &str, script: &str, jit: bool) -> String {
    let mut cmd = lua_command();
    cmd.current_dir(dir).arg(format!("--lua={dialect}"));
    if !jit {
        cmd.arg("--no-jit");
    }
    // the recordings' PUC ran without them: they replace the default paths
    for (k, _) in std::env::vars_os() {
        let name = k.to_string_lossy().to_uppercase();
        if ["LUA_PATH", "LUA_CPATH", "LUA_INIT"].iter().any(|p| name.starts_with(p)) {
            cmd.env_remove(&k);
        }
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
} else if cfg!(windows) {
    "windows"
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
        // a recording of PUC on this platform where it differs from the
        // one on Linux: the C library's BUFSIZ (the room 5.1's and 5.2's
        // buffers have before they allocate), and on Windows the default
        // package paths, which name the executable's directory
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

/// `package.path`, `package.cpath` and `package.config` are PUC's (on
/// Windows naming the directory PUC ran from, where the copy runs).
#[test]
fn package_paths() {
    for d in ["5.1", "5.2", "5.3", "5.4", "5.5"] {
        check_in("tests/package_paths", d, &[false]);
    }
}

/// What lua.c's message handler makes counts as PUC's does: an error
/// object that is no string is described with `lua_pushfstring`.
#[test]
fn lua_c_message_handler() {
    for d in ["5.3", "5.4", "5.5"] {
        check_in("tests/lua_c", d, &[false]);
    }
}
