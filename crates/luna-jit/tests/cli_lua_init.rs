//! `LUA_INIT` (from 5.2 on `LUA_INIT_5_x` first), run by the `luna` CLI
//! where each dialect's `lua.c` runs it.
//!
//! Every expectation was recorded from the stock PUC interpreters — Lua
//! 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1, each built with `make linux` on
//! Linux x86_64 — run as `lua <args>` from a directory holding the case's
//! files; `cli_common` says how luna's output is compared.

mod cli_common;

use cli_common::{Case, Expect};

/// `LUA_INIT` runs before the options' chunks; 5.3 on have created `arg`.
#[test]
fn lua_init_chunk() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-e", "print(1)"],
        stdin: None,
        env: &[("LUA_INIT", "print(\"init\", arg and #arg)")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1", "5.2"],
            stdout: "init\tnil\n1\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4", "5.5"],
            stdout: "init\t2\n1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// From 5.2 on `LUA_INIT_5_x` is taken over `LUA_INIT`.
#[test]
fn lua_init_versioned() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-e", "print(1)"],
        stdin: None,
        env: &[
            ("LUA_INIT", "print(\"init\")"),
            ("LUA_INIT_5_1", "print(\"5.1\")"),
            ("LUA_INIT_5_2", "print(\"5.2\")"),
            ("LUA_INIT_5_3", "print(\"5.3\")"),
            ("LUA_INIT_5_4", "print(\"5.4\")"),
            ("LUA_INIT_5_5", "print(\"5.5\")"),
        ],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "init\n1\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "5.2\n1\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "5.3\n1\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.4"],
            stdout: "5.4\n1\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "5.5\n1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `LUA_INIT=@file` runs the file, with no arguments.
#[test]
fn lua_init_file() {
    let case = Case {
        files: &[
            ("s.lua", "print(\"s\", ...)\n"),
            ("init.lua", "print(\"init\", ...)\n"),
        ],
        args: &["s.lua", "a"],
        stdin: None,
        env: &[("LUA_INIT", "@init.lua")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "init\ns\ta\n",
        stderr: "",
        status: 0,
    }]);
}

/// A missing `@file` is reported and stops the interpreter.
///
/// The reason is the C library's `strerror` text, in its POSIX wording.
#[cfg(unix)]
#[test]
fn lua_init_missing_file() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "@missing.lua")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "",
        stderr: "lua: cannot open missing.lua: No such file or directory\n",
        status: 1,
    }]);
}

/// `@` alone names the file ``.
///
/// The reason is the C library's `strerror` text, in its POSIX wording.
#[cfg(unix)]
#[test]
fn lua_init_empty_file_name() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "@")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "",
        stderr: "lua: cannot open : No such file or directory\n",
        status: 1,
    }]);
}

/// An error in the `@file` is reported with its traceback.
#[test]
fn lua_init_file_error() {
    let case = Case {
        files: &[
            ("s.lua", "print(\"s\", ...)\n"),
            ("init.lua", "error({})\n"),
        ],
        args: &["s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "@init.lua")],
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
            stderr: "lua: (error object is a table value)\nstack traceback:\n\t[C]: in function 'error'\n\tinit.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: (error object is a table value)\nstack traceback:\n\t[C]: in global 'error'\n\tinit.lua:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

/// An error in the chunk: the chunk is named after the variable.
#[test]
fn lua_init_error() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "error('ie')")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: LUA_INIT:1: ie\nstack traceback:\n\t[C]: in function 'error'\n\tLUA_INIT:1: in main chunk\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "",
            stderr: "lua: LUA_INIT:1: ie\nstack traceback:\n\t[C]: in function 'error'\n\tLUA_INIT:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: LUA_INIT:1: ie\nstack traceback:\n\t[C]: in global 'error'\n\tLUA_INIT:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

/// The chunk from `LUA_INIT_5_x` is named after that variable.
#[test]
fn lua_init_versioned_error() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["s.lua"],
        stdin: None,
        env: &[
            ("LUA_INIT_5_2", "error('ie')"),
            ("LUA_INIT_5_3", "error('ie')"),
            ("LUA_INIT_5_4", "error('ie')"),
            ("LUA_INIT_5_5", "error('ie')"),
            ("LUA_INIT", "print('plain')"),
        ],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "plain\ns\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "",
            stderr: "lua: LUA_INIT_5_2:1: ie\nstack traceback:\n\t[C]: in function 'error'\n\tLUA_INIT_5_2:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: LUA_INIT_5_3:1: ie\nstack traceback:\n\t[C]: in function 'error'\n\tLUA_INIT_5_3:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4"],
            stdout: "",
            stderr: "lua: LUA_INIT_5_4:1: ie\nstack traceback:\n\t[C]: in function 'error'\n\tLUA_INIT_5_4:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "",
            stderr: "lua: LUA_INIT_5_5:1: ie\nstack traceback:\n\t[C]: in global 'error'\n\tLUA_INIT_5_5:1: in main chunk\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}

/// A chunk that does not compile.
#[test]
fn lua_init_syntax_error() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "x = = 1")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "",
        stderr: "lua: LUA_INIT:1: unexpected symbol near '='\n",
        status: 1,
    }]);
}

/// `-E` (5.2 on) skips `LUA_INIT`.
#[test]
fn lua_init_ignored_with_e() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-E", "s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "print('init')")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "init\n",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "s\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// 5.1 runs `LUA_INIT` before it looks at the options.
#[test]
fn lua_init_before_usage() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-x"],
        stdin: None,
        env: &[("LUA_INIT", "print('init')")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "init\n",
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

/// The script sees what `LUA_INIT` set.
#[test]
fn lua_init_sets_global() {
    let case = Case {
        files: &[("x.lua", "print(x)\n")],
        args: &["x.lua"],
        stdin: None,
        env: &[("LUA_INIT", "x = 'from init'")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "from init\n",
        stderr: "",
        status: 0,
    }]);
}

/// The version line comes before `LUA_INIT` (after it in 5.1).
#[test]
fn lua_init_after_version() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-v"],
        stdin: None,
        env: &[("LUA_INIT", "print('init')")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "init\n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\ninit\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `LUA_INIT` runs before a program read from stdin.
#[test]
fn lua_init_then_stdin() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &[],
        stdin: Some("print(1)\n"),
        env: &[("LUA_INIT", "print('init')")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "init\n1\n",
        stderr: "",
        status: 0,
    }]);
}

/// What `LUA_INIT` returns is dropped.
#[test]
fn lua_init_results_not_printed() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["s.lua"],
        stdin: None,
        env: &[("LUA_INIT", "return 5")],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "s\n",
        stderr: "",
        status: 0,
    }]);
}
