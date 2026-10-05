//! The order in which a script's standard output and its error messages
//! reach one pipe, file or terminal that both are sent to.
//!
//! PUC's `lua` writes stdout through C stdio, which buffers it (line by
//! line on a terminal, in blocks otherwise), while stderr is unbuffered;
//! `print` flushes from 5.2 on, `io.write` never does. `expected.txt` was
//! recorded from the stock PUC 5.1.5 to 5.5.1 interpreters on Linux x86_64
//! (glibc), run as `lua <case>.lua` with stdout and stderr on the same pipe,
//! the same file, and the same terminal. Runs of 20 or more equal bytes are
//! written `<c*N>` on both sides.
//!
//! Cases whose output fills a buffer depend on the buffer size, which
//! glibc takes from the descriptor; they and the terminal runs are checked
//! on Linux only.
//!
//! On Windows `lua.exe` writes through the MSVC C library instead (4096-byte
//! buffers, `"line"` meaning full buffering, text mode), and
//! `expected-windows.txt` was recorded from PUC 5.1.5 to 5.5.0 built with
//! MSVC on windows-latest, the same way; a carriage return is written
//! `<CR>` there. Every case is checked on Windows.

use crate::cli_common::{DIALECTS, luna, workdir};
use std::process::{Command, Stdio};

const EXPECTED: &str = if cfg!(windows) {
    include_str!("cli_output_order/expected-windows.txt")
} else {
    include_str!("cli_output_order/expected.txt")
};

const CASES: [(&str, &str); 8] = [
    ("a.lua", include_str!("cli_output_order/a.lua")),
    ("b.lua", include_str!("cli_output_order/b.lua")),
    ("c.lua", include_str!("cli_output_order/c.lua")),
    ("d.lua", include_str!("cli_output_order/d.lua")),
    ("e.lua", include_str!("cli_output_order/e.lua")),
    ("f.lua", include_str!("cli_output_order/f.lua")),
    ("g.lua", include_str!("cli_output_order/g.lua")),
    ("h.lua", include_str!("cli_output_order/h.lua")),
];

/// The cases whose output reaches a buffer's size.
const BUFFER_SIZED: [&str; 2] = ["d.lua", "f.lua"];

fn expected(dialect: &str, name: &str) -> String {
    // a Windows checkout may give the recording CRLF line endings
    let all = EXPECTED.replace("\r\n", "\n");
    let head = format!("=== {dialect} {name}\n");
    let start = all.find(&head).unwrap_or_else(|| panic!("no {head}")) + head.len();
    let len = all[start..].find("\n=== end\n").expect("section end");
    all[start..start + len].to_string()
}

/// Runs of 20 or more equal bytes as `<c*N>`, the program's path as `lua`.
fn normalize(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out).replace(luna().to_str().unwrap(), "lua");
    let text = if cfg!(windows) {
        text.replace('\r', "<CR>")
    } else {
        text
    };
    let chars: Vec<char> = text.chars().collect();
    let mut s = String::new();
    let mut i = 0;
    while i < chars.len() {
        let mut j = i;
        while j < chars.len() && chars[j] == chars[i] {
            j += 1;
        }
        if j - i >= 20 {
            s.push_str(&format!("<{}*{}>", chars[i], j - i));
        } else {
            s.extend(&chars[i..j]);
        }
        i = j;
    }
    s
}

fn command(dialect: &str, dir: &std::path::Path, script: &str) -> Command {
    let mut cmd = Command::new(luna());
    cmd.arg(format!("--lua={dialect}"))
        .arg(script)
        .current_dir(dir);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("LUA_") {
            cmd.env_remove(k);
        }
    }
    cmd
}

/// stdout and stderr on one pipe; the output and the exit status line.
fn on_pipe(mut cmd: Command) -> String {
    let (mut reader, writer) = std::io::pipe().expect("pipe");
    cmd.stdin(Stdio::null())
        .stdout(writer.try_clone().expect("dup pipe"))
        .stderr(writer);
    let mut child = cmd.spawn().expect("spawn luna");
    // the parent's copies of the write end go with `cmd`
    drop(cmd);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut out).expect("read pipe");
    let status = child.wait().expect("wait").code().expect("exit code");
    format!("{}status {status}\n", normalize(&out))
}

/// stdout and stderr on one regular file.
fn on_file(mut cmd: Command, path: &std::path::Path) -> String {
    let file = std::fs::File::create(path).expect("create output file");
    cmd.stdin(Stdio::null())
        .stdout(file.try_clone().expect("dup file"))
        .stderr(file);
    let status = cmd.status().expect("run luna").code().expect("exit code");
    let out = std::fs::read(path).expect("read output file");
    format!("{}status {status}\n", normalize(&out))
}

/// stdout and stderr on one terminal: util-linux `script` runs the
/// command on a pseudo-terminal and copies what it writes.
#[cfg(target_os = "linux")]
fn on_terminal(dialect: &str, dir: &std::path::Path, script: &str) -> String {
    let line = format!("'{}' --lua={dialect} {script}", luna().display());
    let out = Command::new("script")
        .args(["-qec", &line, "/dev/null"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("TERM", "dumb")
        .output()
        .expect("run script(1)");
    let status = out.status.code().expect("exit code");
    let text: Vec<u8> = out.stdout.into_iter().filter(|&b| b != b'\r').collect();
    format!("{}status {status}\n", normalize(&text))
}

#[test]
fn script_output_and_errors_interleave_as_in_puc() {
    let dir = workdir(&CASES);
    let mut failed = Vec::new();
    for (name, _) in CASES {
        if !cfg!(any(target_os = "linux", windows)) && BUFFER_SIZED.contains(&name) {
            continue;
        }
        let script = name.to_string();
        let case = name.strip_suffix(".lua").unwrap();
        for d in DIALECTS {
            #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
            let mut runs = vec![
                ("pipe", on_pipe(command(d, &dir, &script))),
                (
                    "file",
                    on_file(command(d, &dir, &script), &dir.join("out.txt")),
                ),
            ];
            #[cfg(target_os = "linux")]
            runs.push(("pty", on_terminal(d, &dir, &script)));
            for (mode, got) in runs {
                let want = expected(d, &format!("{case}.{mode}"));
                if got != want {
                    failed.push(format!(
                        "--lua={d} {script} on a {mode}:\n{got}--- PUC:\n{want}"
                    ));
                }
            }
        }
    }
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The REPL reading a piped stdin: prompts, results and errors in order.
#[test]
fn repl_output_and_errors_interleave_as_in_puc() {
    let dir = workdir(&[]);
    let input = include_str!("cli_output_order/repl-order.txt");
    for d in DIALECTS {
        let (mut reader, writer) = std::io::pipe().expect("pipe");
        let mut cmd = command(d, &dir, "-i");
        cmd.stdin(Stdio::piped())
            .stdout(writer.try_clone().expect("dup pipe"))
            .stderr(writer);
        let mut child = cmd.spawn().expect("spawn luna");
        drop(cmd);
        let mut stdin = child.stdin.take().expect("stdin");
        std::io::Write::write_all(&mut stdin, input.as_bytes()).expect("write stdin");
        drop(stdin);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut out).expect("read pipe");
        child.wait().expect("wait");
        // the version line is luna's own; the recording left PUC's out
        let got: String = normalize(&out)
            .split_inclusive('\n')
            .filter(|l| !l.starts_with("luna "))
            .collect();
        let want = expected(d, "repl.pipe");
        let want = want.strip_suffix("status 0\n").expect("status line");
        assert_eq!(got, want, "--lua={d}");
    }
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
}
