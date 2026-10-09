//! The cases on the main thread: each dialect's scripts run by the `luna`
//! CLI, with and without the JIT, one process per dialect and mode.

use std::process::Command;

use super::{CASES, PRELUDE};

fn script(v: u8) -> String {
    let mut s = String::new();
    for c in CASES
        .iter()
        .filter(|c| (c.dialects.0..=c.dialects.1).contains(&v))
    {
        s.push_str(&format!(
            "print({:?}, (function() {PRELUDE}{} end)())\n",
            c.name, c.script
        ));
    }
    s
}

fn expected(v: u8) -> String {
    CASES
        .iter()
        .filter(|c| (c.dialects.0..=c.dialects.1).contains(&v))
        .map(|c| match c.name {
            // from 5.4 the parser's error goes through the message handler
            // of the protected call that would catch it: lua.c's, which
            // appends a traceback
            "parser_nesting" if v >= 54 => {
                "parser_nesting\tnil|@ in main chunk\n\t[C]: in ?\n".to_string()
            }
            _ => format!("{}\t{}\n", c.name, (c.expect)(v)),
        })
        .collect()
}

#[test]
fn main_thread() {
    let dir = std::env::temp_dir().join(format!("luna-stack-overflow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut failures = Vec::new();
    for v in 51..=55u8 {
        let file = dir.join(format!("cases{v}.lua"));
        std::fs::write(&file, script(v)).expect("write script");
        for jit in [true, false] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_luna"));
            cmd.arg(format!("--lua={}.{}", v / 10, v % 10));
            if !jit {
                cmd.arg("--no-jit");
            }
            let out = cmd.arg(&file).output().expect("run luna");
            let got = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
            if !out.status.success() || got != expected(v) {
                failures.push(format!(
                    "{v} jit={jit}: {}\nstdout:\n{got}stderr:\n{}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
