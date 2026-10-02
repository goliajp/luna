//! The `luna` CLI reports an uncaught error and sets its exit status the
//! way each dialect's standalone interpreter (`lua.c`) does: the message
//! after the program name, the traceback of the message handler, non-string
//! error objects, load errors, the usage message for a bad option, and
//! which stream each goes to.
//!
//! Every expectation was recorded from the stock PUC interpreters — Lua
//! 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1, each built with its default make —
//! run as `lua <args>` (so argv[0], the program name `lua.c` prints, is
//! `lua`) from a directory holding the case's scripts, with stdin as given
//! (empty when `None`). The macOS (`make macosx`) and Linux x86_64
//! (`make linux`) builds printed the same bytes for every case. luna runs
//! with the same arguments after `--lua=5.x`; its argv[0] is the path of the
//! binary under test, rewritten to `lua` before comparing.
//!
//! Text that depends on the platform: `cannot open <file>: <reason>` ends
//! with the C library's `strerror` text, so `missing_script` compares the
//! reason only where it is the POSIX wording (not on Windows).
//!
//! Text that varies between runs of PUC itself: 5.2's traceback names a
//! library function by whichever of its names `pushglobalfuncname` meets
//! first in hash order (`'require'` or `'_G.require'`, both seen in the
//! recordings). The 5.2 expectations use the short spelling, and luna's 5.2
//! output is folded the same way before comparing.

use crate::cli_common::{Case, DIALECTS, Expect, luna, workdir};
use std::process::{Command, Stdio};

/// argv[0] is the program name as given, not the file's name: lua.c's
/// `progname`.
#[cfg(unix)]
#[test]
fn program_name_is_argv0() {
    use std::os::unix::process::CommandExt;
    let dir = workdir(&[]);
    let out = Command::new(luna())
        .arg0("some/where/lua-x")
        .args(["-e", "error('x', 0)"])
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .expect("run luna");
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("some/where/lua-x: x\n"),
        "stderr: {stderr}"
    );
}

/// PUC 5.4.9 and 5.5.1 print the same for `luna --lua=5.x -- missing.lua`
/// (a file named after an option); the reason after the colon is the C
/// library's `strerror`.
#[test]
fn missing_script() {
    let tail = if cfg!(windows) {
        ""
    } else {
        " No such file or directory\n"
    };
    for d in DIALECTS {
        let out = Case {
            files: &[],
            args: &["missing.lua"],
            stdin: None,
            env: &[],
        }
        .run(d);
        // every PUC version: `lua: cannot open missing.lua: No such file or
        // directory`, exit status 1
        assert!(
            out.stderr.starts_with("lua: cannot open missing.lua:") && out.stderr.ends_with(tail),
            "--lua={d}: {}",
            out.stderr
        );
        assert_eq!(out.stdout, "", "--lua={d}");
        assert_eq!(out.status, 1, "--lua={d}");
    }
}

#[test]
fn error_in_nested_functions() {
    let case = Case {
        files: &[(
            "err.lua",
            "local function inner()\n  error(\"boom\")\nend\nlocal function outer() inner() end\nouter()\n",
        )],
        args: &["err.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: err.lua:2: boom\nstack traceback:\n\t[C]: in function 'error'\n\terr.lua:2: in function 'inner'\n\terr.lua:4: in function 'outer'\n\terr.lua:5: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: err.lua:2: boom\nstack traceback:\n\t[C]: in function 'error'\n\terr.lua:2: in function 'inner'\n\terr.lua:4: in function 'outer'\n\terr.lua:5: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "",
            stderr: "lua: err.lua:2: boom\nstack traceback:\n\t[C]: in function 'error'\n\terr.lua:2: in upvalue 'inner'\n\terr.lua:4: in local 'outer'\n\terr.lua:5: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: err.lua:2: boom\nstack traceback:\n\t[C]: in global 'error'\n\terr.lua:2: in upvalue 'inner'\n\terr.lua:4: in local 'outer'\n\terr.lua:5: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn runtime_error() {
    let case = Case {
        files: &[("rt.lua", "local t = nil\nprint(t.x)\n")],
        args: &["rt.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: rt.lua:2: attempt to index local 't' (a nil value)\nstack traceback:\n\trt.lua:2: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: rt.lua:2: attempt to index local 't' (a nil value)\nstack traceback:\n\trt.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4", "5.5"],
            stdout: "",
            stderr: "lua: rt.lua:2: attempt to index a nil value (local 't')\nstack traceback:\n\trt.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn error_level_0() {
    let case = Case {
        files: &[("lvl0.lua", "error(\"plain\", 0)\n")],
        args: &["lvl0.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: plain\nstack traceback:\n\t[C]: in function 'error'\n\tlvl0.lua:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: plain\nstack traceback:\n\t[C]: in function 'error'\n\tlvl0.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: plain\nstack traceback:\n\t[C]: in global 'error'\n\tlvl0.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

mod error_objects;
mod exit_and_options;
mod libraries_and_warnings;
mod load_and_input;
