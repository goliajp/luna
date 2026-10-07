//! `seek` on the standard streams of the `luna` command on Windows, where
//! the MSVC C library's `fseek` and `ftell` go to the system for standard
//! input as for any file. Standard input is set up here, the same way for
//! luna and for the recording: a file, `NUL`, an anonymous pipe that holds
//! the whole input and whose writer is closed before the program starts
//! (so what the system reports for it does not depend on timing), and a
//! pipe whose writer has written nothing and is still open. The first
//! three are held to what PUC 5.1.5 to 5.5.0 built with MSVC gave on
//! windows-latest; for the last, where the numbers depend on the system,
//! each result must be of the same kind as PUC's: a number, or the same
//! failure.
//!
//! With `LUNA_STDIN_SEEK_PUC` naming the directory of PUC's builds
//! (`lua-5.x.y\src\lua.exe`) and `LUNA_STDIN_SEEK_RECORD` a directory, the
//! test runs PUC instead and writes what it gave there.
#![cfg(windows)]

use crate::cli_common::{luna, workdir};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SCRIPT: &str = include_str!("../../../luna-core/tests/crt_text/stdinseek.lua");
const OPEN_SCRIPT: &str = include_str!("../../../luna-core/tests/crt_text/stdinseek_open.lua");
const PUC: [&str; 5] = [
    include_str!("../../../luna-core/tests/crt_text/stdinseek.5.1.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek.5.2.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek.5.3.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek.5.4.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek.5.5.txt"),
];
const PUC_OPEN: [&str; 5] = [
    include_str!("../../../luna-core/tests/crt_text/stdinseek_open.5.1.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek_open.5.2.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek_open.5.3.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek_open.5.4.txt"),
    include_str!("../../../luna-core/tests/crt_text/stdinseek_open.5.5.txt"),
];
const DIALECTS: [(&str, &str); 5] = [
    ("5.1", "5.1.5"),
    ("5.2", "5.2.4"),
    ("5.3", "5.3.6"),
    ("5.4", "5.4.8"),
    ("5.5", "5.5.0"),
];
const INPUT: &[u8] = b"line one\nline two\n";

/// Standard input of one run.
enum Input {
    File,
    Nul,
    /// a pipe holding all of `INPUT`, its writer closed
    Pipe,
    /// a pipe whose writer has written nothing and stays open
    OpenPipe,
}

/// Run `exe` (with `args`) on `script` with `input` as standard input, its
/// rows appended to `seek.out` under `label`.
fn run(dir: &Path, exe: &Path, args: &[String], script: &str, label: &str, input: Input) {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .args([script, "seek.out", label])
        .current_dir(dir)
        .stdout(File::create(dir.join("so.txt")).expect("stdout file"))
        .stderr(File::create(dir.join("se.txt")).expect("stderr file"));
    let mut writer = None;
    match input {
        Input::File => {
            cmd.stdin(File::open(dir.join("in.txt")).expect("the input file"));
        }
        Input::Nul => {
            cmd.stdin(File::open("NUL").expect("NUL"));
        }
        Input::Pipe => {
            let (r, mut w) = std::io::pipe().expect("a pipe");
            w.write_all(INPUT).expect("fill the pipe");
            drop(w);
            cmd.stdin(Stdio::from(r));
        }
        Input::OpenPipe => {
            let (r, w) = std::io::pipe().expect("a pipe");
            writer = Some(w);
            cmd.stdin(Stdio::from(r));
        }
    }
    let status = cmd.status().expect("run the program");
    drop(writer);
    let stderr = std::fs::read_to_string(dir.join("se.txt")).unwrap_or_default();
    assert!(
        status.success(),
        "{} {label}: {status}\n{stderr}",
        exe.display()
    );
}

/// The rows each kind of standard input gives, and those of the open pipe.
fn rows(exe: &Path, args: &[String]) -> (String, String) {
    let dir = workdir(&[("seek.lua", SCRIPT), ("open.lua", OPEN_SCRIPT)]);
    std::fs::write(dir.join("in.txt"), INPUT).expect("the input file");
    run(&dir, exe, args, "seek.lua", "file", Input::File);
    run(&dir, exe, args, "seek.lua", "nul", Input::Nul);
    run(&dir, exe, args, "seek.lua", "pipe", Input::Pipe);
    let fixed = std::fs::read_to_string(dir.join("seek.out")).expect("the rows");
    std::fs::remove_file(dir.join("seek.out")).expect("start the rows again");
    run(&dir, exe, args, "open.lua", "open-pipe", Input::OpenPipe);
    let open = std::fs::read_to_string(dir.join("seek.out")).expect("the rows");
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
    (fixed, open)
}

/// What kind of result a row holds: a number, or the failure as it is.
fn kind(row: &str) -> String {
    let (head, value) = row.rsplit_once('\t').expect("a row");
    if value.parse::<f64>().is_ok() {
        format!("{head}\t<number>")
    } else {
        row.to_string()
    }
}

#[test]
fn standard_streams_seek_as_in_puc_built_with_msvc() {
    if let (Ok(puc), Ok(out)) = (
        std::env::var("LUNA_STDIN_SEEK_PUC"),
        std::env::var("LUNA_STDIN_SEEK_RECORD"),
    ) {
        for (d, v) in DIALECTS {
            let exe = PathBuf::from(&puc)
                .join(format!("lua-{v}"))
                .join("src")
                .join("lua.exe");
            let (fixed, open) = rows(&exe, &[]);
            std::fs::write(Path::new(&out).join(format!("stdinseek.{d}.txt")), fixed)
                .expect("record");
            std::fs::write(
                Path::new(&out).join(format!("stdinseek_open.{d}.txt")),
                open,
            )
            .expect("record");
        }
        return;
    }
    for (((d, _), want), want_open) in DIALECTS.into_iter().zip(PUC).zip(PUC_OPEN) {
        let (got, open) = rows(&luna(), &[format!("--lua={d}")]);
        let want = want.replace("\r\n", "\n");
        for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
            assert_eq!(g, w, "--lua={d}, line {}", i + 1);
        }
        assert_eq!(
            got.lines().count(),
            want.lines().count(),
            "--lua={d}: line count"
        );
        let want_open = want_open.replace("\r\n", "\n");
        for (i, (g, w)) in open.lines().zip(want_open.lines()).enumerate() {
            assert_eq!(kind(g), kind(w), "--lua={d}, open pipe, line {}", i + 1);
        }
        assert_eq!(
            open.lines().count(),
            want_open.lines().count(),
            "--lua={d}: open pipe rows"
        );
    }
}
