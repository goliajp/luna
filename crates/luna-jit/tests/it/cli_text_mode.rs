//! On Windows the `luna` command's standard streams and the files it opens
//! without `b` are in the MSVC C library's text mode, as `lua.exe`'s are.
//! The expected bytes were recorded from PUC 5.1.5 to 5.5.0 built with
//! MSVC 19.51 (`cl /O2 /MD`) on windows-latest, run the same way; only the
//! program name and the version line, which are luna's own, are rewritten.
#![cfg(windows)]

use crate::cli_common::{luna, workdir};
use std::io::Write;
use std::process::{Command, Stdio};

const OUT_LUA: &str = "print(\"p1\")
io.write(\"w1\\n\", \"w2\\r\\n\", \"w3\\r\", \"w4\\n\\n\")
io.stdout:write(\"s1\\n\")
io.stderr:write(\"e1\\ne2\\r\\n\")
io.stdout:setvbuf(\"no\"); io.write(\"nb\\n\")
print((\"x\"):rep(3) .. \"\\n\" .. \"y\")
";
const OUT_STDOUT: &[u8] = b"p1\r\nw1\r\nw2\r\r\nw3\rw4\r\n\r\ns1\r\nnb\r\nxxx\r\ny\r\n";
const OUT_STDERR: &[u8] = b"e1\r\ne2\r\r\n";

const ERR_LUA: &str = "print(\"before\")\nerror(\"boom\\nsecond line\")\n";

const REPL_IN: &str = "print(1)\nreturn 2, \"a\\nb\"\nerror(\"e\")\n";
const REPL_STDOUT: &[u8] = b"> 1\r\n> 2\ta\r\nb\r\n> > \r\n";

const FILES_LUA: &str = include_str!("../../../luna-core/tests/crt_text/files.lua");
const FILES_51: &str = include_str!("../../../luna-core/tests/crt_text/files.5.1.txt");
const FILES: &str = include_str!("../../../luna-core/tests/crt_text/files.txt");

const DIALECTS: [&str; 5] = ["5.1", "5.2", "5.3", "5.4", "5.5"];

struct Run {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run(dialect: &str, files: &[(&str, &str)], args: &[&str], stdin: &str) -> Run {
    let dir = workdir(files);
    let mut child = Command::new(luna())
        .arg(format!("--lua={dialect}"))
        .args(args)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn luna");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait for luna");
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
    let progname = luna().to_str().expect("UTF-8 path").as_bytes().to_vec();
    Run {
        stdout: out.stdout,
        stderr: replace(&out.stderr, &progname, b"lua"),
    }
}

fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// The traceback of an uncaught error raised by `error` at `chunk`:2 (or
/// line 1 of stdin), as each dialect's lua.c writes it.
fn traceback(dialect: &str, place: &str) -> String {
    let func = if dialect == "5.5" {
        "global 'error'"
    } else {
        "function 'error'"
    };
    let last = if dialect == "5.1" {
        "[C]: ?"
    } else {
        "[C]: in ?"
    };
    format!("stack traceback:\r\n\t[C]: in {func}\r\n\t{place}: in main chunk\r\n\t{last}\r\n")
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).escape_debug().to_string()
}

#[test]
fn standard_streams_write_crlf() {
    for d in DIALECTS {
        let script = run(d, &[("out.lua", OUT_LUA)], &["out.lua"], "");
        assert_eq!(show(&script.stdout), show(OUT_STDOUT), "--lua={d}");
        assert_eq!(show(&script.stderr), show(OUT_STDERR), "--lua={d}");
        // the same script read from a pipe on standard input
        let piped = run(d, &[], &["-"], OUT_LUA);
        assert_eq!(show(&piped.stdout), show(OUT_STDOUT), "--lua={d} -");
        assert_eq!(show(&piped.stderr), show(OUT_STDERR), "--lua={d} -");
    }
}

#[test]
fn error_messages_write_crlf() {
    for d in DIALECTS {
        let r = run(d, &[("err.lua", ERR_LUA)], &["err.lua"], "");
        assert_eq!(show(&r.stdout), show(b"before\r\n"), "--lua={d}");
        let want = format!(
            "lua: err.lua:2: boom\r\nsecond line\r\n{}",
            traceback(d, "err.lua:2")
        );
        assert_eq!(show(&r.stderr), show(want.as_bytes()), "--lua={d}");
    }
}

#[test]
fn interactive_mode_writes_crlf() {
    for d in DIALECTS {
        let r = run(d, &[], &["-i"], REPL_IN);
        // after the version line, which is luna's own (stderr in 5.1)
        let (stdout, stderr) = if d == "5.1" {
            let nl = r
                .stderr
                .iter()
                .position(|&b| b == b'\n')
                .expect("version line");
            (r.stdout.clone(), r.stderr[nl + 1..].to_vec())
        } else {
            let nl = r
                .stdout
                .iter()
                .position(|&b| b == b'\n')
                .expect("version line");
            (r.stdout[nl + 1..].to_vec(), r.stderr.clone())
        };
        assert_eq!(show(&stdout), show(REPL_STDOUT), "--lua={d}");
        let want = format!("stdin:1: e\r\n{}", traceback(d, "stdin:1"));
        assert_eq!(show(&stderr), show(want.as_bytes()), "--lua={d}");
    }
}

#[test]
fn files_read_and_write_as_in_lua_exe() {
    for d in DIALECTS {
        let want = if d == "5.1" { FILES_51 } else { FILES };
        let want = want.replace("\r\n", "\n").replace('\n', "\r\n");
        let r = run(d, &[("files.lua", FILES_LUA)], &["files.lua"], "");
        assert_eq!(show(&r.stderr), "", "--lua={d}");
        assert_eq!(show(&r.stdout), show(want.as_bytes()), "--lua={d}");
    }
}
