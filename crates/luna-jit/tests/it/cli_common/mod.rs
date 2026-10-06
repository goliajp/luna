//! Running the `luna` CLI against what the stock PUC interpreters printed
//! for the same command line, stdin and environment.
//!
//! luna runs with the case's arguments after `--lua=5.x`, in a fresh
//! directory holding the case's files, with every `LUA_*` variable of the
//! test's own environment removed and the case's set. Before comparing,
//! the path of the binary under test (its argv[0], which `lua.c` prints as
//! the program name) is rewritten to `lua`, as PUC was run. The version
//! line is luna's own where `lua.c` prints the release's copyright line;
//! the recordings have the copyright line as `<version>`, and luna's line
//! is rewritten to that. 5.2's traceback names a library function by
//! whichever of its names `pushglobalfuncname` meets first in hash order
//! (`'require'` or `'_G.require'`, both seen in the recordings); the 5.2
//! expectations use the short spelling, and luna's 5.2 output is folded
//! the same way.

#![allow(dead_code)] // each test file uses its own part of this

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const DIALECTS: [&str; 5] = ["5.1", "5.2", "5.3", "5.4", "5.5"];

pub struct Case {
    /// Files written into the working directory: (name, contents).
    pub files: &'static [(&'static str, &'static str)],
    /// Arguments after the program name (and luna's `--lua=`).
    pub args: &'static [&'static str],
    /// stdin (empty when `None`); never a terminal.
    pub stdin: Option<&'static str>,
    /// Environment variables set for the run.
    pub env: &'static [(&'static str, &'static str)],
}

/// What PUC printed for some dialects.
pub struct Expect {
    pub dialects: &'static [&'static str],
    pub stdout: &'static str,
    pub stderr: &'static str,
    pub status: i32,
}

pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

/// What PUC built with MSVC writes for `text` (recorded from a run on
/// Linux): its standard streams are in the C library's text mode on
/// Windows, so each `\n` goes out as `\r\n`.
pub fn as_on_this_platform(text: &str) -> String {
    if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

pub fn luna() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_luna"))
}

/// A fresh working directory holding `files`.
pub fn workdir(files: &[(&str, &str)]) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "luna-cli-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("remove a stale work dir");
    }
    std::fs::create_dir_all(&dir).expect("create the work dir");
    for (name, body) in files {
        // written byte for byte: no line-ending conversion on any platform
        std::fs::write(dir.join(name), body.as_bytes()).expect("write a file");
    }
    dir
}

pub fn run(
    dialect: &str,
    dir: &Path,
    args: &[&str],
    stdin: Option<&str>,
    env: &[(&str, &str)],
) -> Output {
    let bin = luna();
    let mut cmd = Command::new(&bin);
    cmd.arg(format!("--lua={dialect}"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // what lua.c would read from the environment (LUA_PATH, LUA_INIT, ...)
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("LUA_") {
            cmd.env_remove(k);
        }
    }
    cmd.envs(env.iter().copied());
    let mut child = cmd.spawn().expect("spawn luna");
    let mut input = child.stdin.take().expect("piped stdin");
    // a run that never reads stdin may exit before the write lands
    match input.write_all(stdin.unwrap_or_default().as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        r => r.expect("write stdin"),
    }
    drop(input);
    let out = child.wait_with_output().expect("wait for luna");
    let progname = bin.to_str().expect("UTF-8 binary path");
    let version = format!("luna {} (Lua {dialect})", env!("CARGO_PKG_VERSION"));
    let normalize = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace(&version, "<version>");
    let mut stderr = normalize(&out.stderr).replace(progname, "lua");
    if dialect == "5.2" {
        stderr = stderr.replace("'_G.", "'");
    }
    Output {
        stdout: normalize(&out.stdout),
        stderr,
        status: out.status.code().expect("luna exited, not killed"),
    }
}

impl Case {
    pub fn run(&self, dialect: &str) -> Output {
        let dir = workdir(self.files);
        let out = run(dialect, &dir, self.args, self.stdin, self.env);
        std::fs::remove_dir_all(&dir).expect("remove the work dir");
        out
    }

    /// Compare every dialect, each named once in `expects`.
    pub fn expect(&self, expects: &[Expect]) {
        let covered: Vec<&str> = expects
            .iter()
            .flat_map(|e| e.dialects.iter().copied())
            .collect();
        assert_eq!(covered.len(), DIALECTS.len(), "every dialect once");
        for d in DIALECTS {
            assert!(covered.contains(&d), "no expectation for {d}");
        }
        self.expect_dialects(expects);
    }

    /// Compare the dialects `expects` names.
    pub fn expect_dialects(&self, expects: &[Expect]) {
        for e in expects {
            for d in e.dialects {
                let out = self.run(d);
                assert_eq!(
                    out.stderr,
                    as_on_this_platform(e.stderr),
                    "stderr, --lua={d}"
                );
                assert_eq!(
                    out.stdout,
                    as_on_this_platform(e.stdout),
                    "stdout, --lua={d}"
                );
                assert_eq!(out.status, e.status, "exit status, --lua={d}");
            }
        }
    }
}
