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

#[test]
fn error_number() {
    let case = Case {
        files: &[("num.lua", "error(42)\n")],
        args: &["num.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: num.lua:1: 42\nstack traceback:\n\t[C]: in function 'error'\n\tnum.lua:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: num.lua:1: 42\nstack traceback:\n\t[C]: in function 'error'\n\tnum.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "",
            stderr: "lua: 42\nstack traceback:\n\t[C]: in function 'error'\n\tnum.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: 42\nstack traceback:\n\t[C]: in global 'error'\n\tnum.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn error_table() {
    let case = Case {
        files: &[("tbl.lua", "error({})\n")],
        args: &["tbl.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: (error object is not a string)\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: (no error message)\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "",
            stderr: "lua: (error object is a table value)\nstack traceback:\n\t[C]: in function 'error'\n\ttbl.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: (error object is a table value)\nstack traceback:\n\t[C]: in global 'error'\n\ttbl.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn error_nil() {
    let case = Case {
        files: &[("nilerr.lua", "error()\n")],
        args: &["nilerr.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1", "5.2"],
            stdout: "",
            stderr: "",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "",
            stderr: "lua: (error object is a nil value)\nstack traceback:\n\t[C]: in function 'error'\n\tnilerr.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: (error object is a nil value)\nstack traceback:\n\t[C]: in global 'error'\n\tnilerr.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn error_tostring() {
    let case = Case {
        files: &[(
            "ts.lua",
            "error(setmetatable({}, {__tostring = function() return \"custom\" end}))\n",
        )],
        args: &["ts.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: (error object is not a string)\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "",
            stderr: "lua: custom\n",
            status: 1,
        },
    ]);
}

#[test]
fn error_tostring_not_string() {
    let case = Case {
        files: &[(
            "tsnum.lua",
            "error(setmetatable({}, {__tostring = function() return 7 end}))\n",
        )],
        args: &["tsnum.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: (error object is not a string)\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: 7\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "",
            stderr: "lua: (error object is a table value)\nstack traceback:\n\t[C]: in function 'error'\n\ttsnum.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: (error object is a table value)\nstack traceback:\n\t[C]: in global 'error'\n\ttsnum.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

/// A `__tostring` that raises while the message handler runs it.
#[test]
fn error_tostring_raises() {
    let case = Case {
        files: &[(
            "tserr.lua",
            "error(setmetatable({}, {__tostring = function() error(\"in tostring\") end}))\n",
        )],
        args: &["tserr.lua"],
        stdin: None,
        env: &[],
    };
    case.expect_dialects(&[Expect {
        dialects: &["5.1"],
        stdout: "",
        stderr: "lua: (error object is not a string)\n",
        status: 1,
    }]);
}

/// 5.2 on: the error `__tostring` raises inside the handler calls the
/// handler again where it was raised (PUC `luaG_errormsg`), so the final
/// traceback holds the handler's own frames too: the `__tostring` function
/// and the first handler run's `[C]: in ?`.
#[test]
fn error_tostring_raises_in_handler() {
    let case = Case {
        files: &[(
            "tserr.lua",
            "error(setmetatable({}, {__tostring = function() error(\"in tostring\") end}))\n",
        )],
        args: &["tserr.lua"],
        stdin: None,
        env: &[],
    };
    case.expect_dialects(&[
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: tserr.lua:1: in tostring\nstack traceback:\n\t[C]: in function 'error'\n\ttserr.lua:1: in function <tserr.lua:1>\n\t[C]: in ?\n\t[C]: in function 'error'\n\ttserr.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: tserr.lua:1: in tostring\nstack traceback:\n\t[C]: in global 'error'\n\ttserr.lua:1: in function <tserr.lua:1>\n\t[C]: in ?\n\t[C]: in global 'error'\n\ttserr.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn syntax_error() {
    let case = Case {
        files: &[("syn.lua", "local x = = 1\n")],
        args: &["syn.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "",
        stderr: "lua: syn.lua:1: unexpected symbol near '='\n",
        status: 1,
    }]);
}

#[test]
fn inline_error() {
    let case = Case {
        files: &[],
        args: &["-e", "error(\"x\")"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: (command line):1: x\nstack traceback:\n\t[C]: in function 'error'\n\t(command line):1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: (command line):1: x\nstack traceback:\n\t[C]: in function 'error'\n\t(command line):1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: (command line):1: x\nstack traceback:\n\t[C]: in global 'error'\n\t(command line):1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn inline_syntax_error() {
    let case = Case {
        files: &[],
        args: &["-e", "x = = 1"],
        stdin: None,
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "",
        stderr: "lua: (command line):1: unexpected symbol near '='\n",
        status: 1,
    }]);
}

#[test]
fn inline_then_script() {
    let case = Case {
        files: &[("ok.lua", "print(\"script\", ...)\n")],
        args: &["-e", "print(\"inline\")", "ok.lua", "a", "b"],
        stdin: None,
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "inline\nscript\ta\tb\n",
        stderr: "",
        status: 0,
    }]);
}

#[test]
fn stdin_dash_error() {
    let case = Case {
        files: &[],
        args: &["-"],
        stdin: Some("error(\"s\")\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: stdin:1: s\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: stdin:1: s\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: stdin:1: s\nstack traceback:\n\t[C]: in global 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn stdin_implicit_error() {
    let case = Case {
        files: &[],
        args: &[],
        stdin: Some("error(\"s\")\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: stdin:1: s\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: ?\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: stdin:1: s\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: stdin:1: s\nstack traceback:\n\t[C]: in global 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n",
            status: 0,
        },
    ]);
}

#[test]
fn stdout_then_error() {
    let case = Case {
        files: &[("out.lua", "io.write(\"before\\n\")\nerror(\"after\")\n")],
        args: &["out.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "before\n",
            stderr: "lua: out.lua:2: after\nstack traceback:\n\t[C]: in function 'error'\n\tout.lua:2: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "before\n",
            stderr: "lua: out.lua:2: after\nstack traceback:\n\t[C]: in function 'error'\n\tout.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "before\n",
            stderr: "lua: out.lua:2: after\nstack traceback:\n\t[C]: in global 'error'\n\tout.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn os_exit_code() {
    let case = Case {
        files: &[("exit3.lua", "io.write(\"out\\n\")\nos.exit(3)\n")],
        args: &["exit3.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "out\n",
        stderr: "",
        status: 3,
    }]);
}

#[test]
fn os_exit_false() {
    let case = Case {
        files: &[("exitf.lua", "os.exit(false)\n")],
        args: &["exitf.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: exitf.lua:1: bad argument #1 to 'exit' (number expected, got boolean)\nstack traceback:\n\t[C]: in function 'exit'\n\texitf.lua:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "",
            stderr: "",
            status: 1,
        },
    ]);
}

#[test]
fn os_exit_true() {
    let case = Case {
        files: &[("exitt.lua", "os.exit(true)\n")],
        args: &["exitt.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: exitt.lua:1: bad argument #1 to 'exit' (number expected, got boolean)\nstack traceback:\n\t[C]: in function 'exit'\n\texitt.lua:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "",
            stderr: "",
            status: 0,
        },
    ]);
}

#[test]
fn os_exit_bad_arg() {
    let case = Case {
        files: &[("exitbad.lua", "os.exit(\"x\")\n")],
        args: &["exitbad.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: exitbad.lua:1: bad argument #1 to 'exit' (number expected, got string)\nstack traceback:\n\t[C]: in function 'exit'\n\texitbad.lua:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: exitbad.lua:1: bad argument #1 to 'exit' (number expected, got string)\nstack traceback:\n\t[C]: in function 'exit'\n\texitbad.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "",
            stderr: "lua: exitbad.lua:1: bad argument #1 to 'exit' (number expected, got string)\nstack traceback:\n\t[C]: in function 'os.exit'\n\texitbad.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: exitbad.lua:1: bad argument #1 to 'exit' (number expected, got string)\nstack traceback:\n\t[C]: in field 'exit'\n\texitbad.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn bad_option() {
    let case = Case {
        files: &[],
        args: &["-x"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: unrecognized option '-x'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: unrecognized option '-x'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: unrecognized option '-x'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat   execute string 'stat'\n  -i        enter interactive mode after executing 'script'\n  -l mod    require library 'mod' into global 'mod'\n  -l g=mod  require library 'mod' into global 'g'\n  -v        show version information\n  -E        ignore environment variables\n  -W        turn warnings on\n  --        stop handling options\n  -         stop handling options and execute stdin\n",
            status: 1,
        },
    ]);
}

#[test]
fn option_needs_argument() {
    let case = Case {
        files: &[],
        args: &["-e"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: '-e' needs argument\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: '-e' needs argument\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: '-e' needs argument\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat   execute string 'stat'\n  -i        enter interactive mode after executing 'script'\n  -l mod    require library 'mod' into global 'mod'\n  -l g=mod  require library 'mod' into global 'g'\n  -v        show version information\n  -E        ignore environment variables\n  -W        turn warnings on\n  --        stop handling options\n  -         stop handling options and execute stdin\n",
            status: 1,
        },
    ]);
}

#[test]
fn bad_long_option() {
    let case = Case {
        files: &[],
        args: &["--foo"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: unrecognized option '--foo'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: unrecognized option '--foo'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: unrecognized option '--foo'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat   execute string 'stat'\n  -i        enter interactive mode after executing 'script'\n  -l mod    require library 'mod' into global 'mod'\n  -l g=mod  require library 'mod' into global 'g'\n  -v        show version information\n  -E        ignore environment variables\n  -W        turn warnings on\n  --        stop handling options\n  -         stop handling options and execute stdin\n",
            status: 1,
        },
    ]);
}

#[test]
fn library_error() {
    let case = Case {
        files: &[("errmod.lua", "error(\"in module\")\n")],
        args: &["-l", "errmod"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: ./errmod.lua:1: in module\nstack traceback:\n\t[C]: in function 'error'\n\t./errmod.lua:1: in main chunk\n\t[C]: ?\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: ./errmod.lua:1: in module\nstack traceback:\n\t[C]: in function 'error'\n\t./errmod.lua:1: in main chunk\n\t[C]: in function 'require'\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: ./errmod.lua:1: in module\nstack traceback:\n\t[C]: in global 'error'\n\t./errmod.lua:1: in main chunk\n\t[C]: in function 'require'\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn library_not_callable() {
    let case = Case {
        files: &[],
        args: &["-e", "require = nil", "-l", "foo"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: attempt to call a nil value\nstack traceback:\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "",
            stderr: "lua: attempt to call a nil value\nstack traceback:\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn no_debug_library() {
    let case = Case {
        files: &[("nodebug.lua", "debug = nil\nerror(\"bare\")\n")],
        args: &["nodebug.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: nodebug.lua:2: bare\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: nodebug.lua:2: bare\nstack traceback:\n\t[C]: in function 'error'\n\tnodebug.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: nodebug.lua:2: bare\nstack traceback:\n\t[C]: in global 'error'\n\tnodebug.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

#[test]
fn warnings_off_by_default() {
    let case = Case {
        files: &[("warn.lua", "warn(\"hi\")\nprint(\"done\")\n")],
        args: &["warn.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: warn.lua:1: attempt to call global 'warn' (a nil value)\nstack traceback:\n\twarn.lua:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: warn.lua:1: attempt to call global 'warn' (a nil value)\nstack traceback:\n\twarn.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: warn.lua:1: attempt to call a nil value (global 'warn')\nstack traceback:\n\twarn.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "done\n",
            stderr: "",
            status: 0,
        },
    ]);
}

#[test]
fn warnings_on_with_w_option() {
    let case = Case {
        files: &[("warn.lua", "warn(\"hi\")\nprint(\"done\")\n")],
        args: &["-W", "warn.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: unrecognized option '-W'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: unrecognized option '-W'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "done\n",
            stderr: "Lua warning: hi\n",
            status: 0,
        },
    ]);
}

/// A `debug.debug` command nested too deep for the parser: 5.4+ raise the
/// parser's "C stack overflow" through the running message handler, as
/// `load` does (lua.c's handler adds a traceback, an xpcall handler
/// rewrites it, pcall has none); a command that runs and fails has no
/// handler.
#[test]
fn debug_debug_deep_command() {
    let deep = format!("x={}", "(".repeat(240));
    let stdin = format!("{deep}\ncont\n{deep}\ncont\n{deep}\ncont\n");
    let case = Case {
        files: &[(
            "dd.lua",
            "debug.debug()\nprint(pcall(debug.debug))\nprint(xpcall(debug.debug, function(m) return \"H:\" .. m end))\n",
        )],
        args: &["dd.lua"],
        stdin: Some(Box::leak(stdin.into_boxed_str())),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "true\ntrue\n",
            stderr: "lua_debug> (debug command):1: chunk has too many syntax levels\nlua_debug> lua_debug> (debug command):1: chunk has too many syntax levels\nlua_debug> lua_debug> (debug command):1: chunk has too many syntax levels\nlua_debug> ",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3"],
            stdout: "true\ntrue\n",
            stderr: "lua_debug> (debug command):1: too many C levels (limit is 200) in main function near '('\nlua_debug> lua_debug> (debug command):1: too many C levels (limit is 200) in main function near '('\nlua_debug> lua_debug> (debug command):1: too many C levels (limit is 200) in main function near '('\nlua_debug> ",
            status: 0,
        },
        Expect {
            dialects: &["5.4"],
            stdout: "true\ntrue\n",
            stderr: "lua_debug> C stack overflow\nstack traceback:\n\t[C]: in function 'debug.debug'\n\tdd.lua:1: in main chunk\n\t[C]: in ?\nlua_debug> lua_debug> C stack overflow\nlua_debug> lua_debug> H:C stack overflow\nlua_debug> ",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "true\ntrue\n",
            stderr: "lua_debug> C stack overflow\nstack traceback:\n\t[C]: in field 'debug'\n\tdd.lua:1: in main chunk\n\t[C]: in ?\nlua_debug> lua_debug> C stack overflow\nlua_debug> lua_debug> H:C stack overflow\nlua_debug> ",
            status: 0,
        },
    ]);
}

#[test]
fn error_in_arg_order() {
    let case = Case {
        files: &[(
            "args.lua",
            "print(arg[0], arg[1], select(\"#\", ...))\nerror(arg[1])\n",
        )],
        args: &["args.lua", "boom"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "args.lua\tboom\t1\n",
            stderr: "lua: args.lua:2: boom\nstack traceback:\n\t[C]: in function 'error'\n\targs.lua:2: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "args.lua\tboom\t1\n",
            stderr: "lua: args.lua:2: boom\nstack traceback:\n\t[C]: in function 'error'\n\targs.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "args.lua\tboom\t1\n",
            stderr: "lua: args.lua:2: boom\nstack traceback:\n\t[C]: in global 'error'\n\targs.lua:2: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

/// 5.4 on: the parser's "C stack overflow" is a runtime error raised
/// inside `load`'s protected parser, which keeps the running message
/// handler, so lua.c's handler (or an enclosing xpcall's) turns the
/// message `load` returns into a traceback; under pcall or in a coroutine
/// there is none.
#[test]
fn deep_load() {
    let case = Case {
        files: &[(
            "deep.lua",
            "local src = string.rep(\"(\", 300) .. \"1\" .. string.rep(\")\", 300)\nlocal f, e = load(src, \"=c\") print(e)\nprint(pcall(load, src, \"=c\"))\nprint(xpcall(function() local f, e = load(src, \"=c\"); return \"ret:\" .. tostring(e) end, function(m) return \"H:\" .. m end))\nlocal co = coroutine.wrap(function() local f, e = load(src, \"=c\"); return e end)\nprint(co())\n",
        )],
        args: &["deep.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: deep.lua:2: bad argument #1 to 'load' (function expected, got string)\nstack traceback:\n\t[C]: in function 'load'\n\tdeep.lua:2: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3"],
            stdout: "c:1: too many C levels (limit is 200) in main function near '('\ntrue\tnil\tc:1: too many C levels (limit is 200) in main function near '('\ntrue\tret:c:1: too many C levels (limit is 200) in main function near '('\nc:1: too many C levels (limit is 200) in main function near '('\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.4"],
            stdout: "C stack overflow\nstack traceback:\n\t[C]: in function 'load'\n\tdeep.lua:2: in main chunk\n\t[C]: in ?\ntrue\tnil\tC stack overflow\ntrue\tret:H:C stack overflow\nC stack overflow\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "C stack overflow\nstack traceback:\n\t[C]: in global 'load'\n\tdeep.lua:2: in main chunk\n\t[C]: in ?\ntrue\tnil\tC stack overflow\ntrue\tret:H:C stack overflow\nC stack overflow\n",
            stderr: "",
            status: 0,
        },
    ]);
}
