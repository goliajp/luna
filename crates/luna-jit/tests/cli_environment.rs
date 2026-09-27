//! What the `luna` CLI takes from the environment, as `lua.c` and the
//! package library do: `LUA_PATH` / `LUA_CPATH` and `-E`, and `-l` in its
//! forms.
//!
//! Every expectation was recorded from the stock PUC interpreters — Lua
//! 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1, each built with `make linux` on
//! Linux x86_64 — run as `lua <args>` from a directory holding the case's
//! files; `cli_common` says how luna's output is compared.

mod cli_common;

use cli_common::{Case, Expect};

/// `LUA_PATH` / `LUA_CPATH` set `package.path` / `cpath`; from 5.2 on
/// `LUA_PATH_5_x` comes first.
#[test]
fn paths_from_environment() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-e", "print(package.path, package.cpath)"],
        stdin: None,
        env: &[
            ("LUA_PATH", "X"),
            ("LUA_CPATH", "C"),
            ("LUA_PATH_5_3", "Y53"),
            ("LUA_PATH_5_5", "Y55"),
        ],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1", "5.2", "5.4"],
            stdout: "X\tC\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "Y53\tC\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "Y55\tC\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-E` (5.2 on) leaves `package.path` / `cpath` at their defaults.
#[test]
fn ignore_environment_paths() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &[
            "-E",
            "-e",
            "print(package.path == 'X', package.cpath == 'C')",
        ],
        stdin: None,
        env: &[("LUA_PATH", "X"), ("LUA_CPATH", "C")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "false\tfalse\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-E` sets the registry's `LUA_NOENV`.
#[test]
fn ignore_environment_registry() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-E", "-e", "print(debug.getregistry().LUA_NOENV)"],
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
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "true\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// Without `-E` the registry has no `LUA_NOENV`.
#[test]
fn no_ignore_environment_registry() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-e", "print(debug.getregistry().LUA_NOENV)"],
        stdin: None,
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "nil\n",
        stderr: "",
        status: 0,
    }]);
}

/// `-E` does not hide the environment from `os.getenv`.
#[test]
fn ignore_environment_keeps_getenv() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-E", "-e", "print(os.getenv('LUA_X'))"],
        stdin: None,
        env: &[("LUA_X", "1")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "usage: lua [options] [script [args]].\nAvailable options are:\n  -e stat  execute string 'stat'\n  -l name  require library 'name'\n  -i       enter interactive mode after executing 'script'\n  -v       show version information\n  --       stop handling options\n  -        execute stdin and stop handling options\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-lmod`: the module name joined to the option.
#[test]
fn require_option_joined() {
    let case = Case {
        files: &[
            ("m.lua", "print(\"loading\", ...)\nreturn {n = 1}\n"),
            ("m-v2.lua", "print(\"loading\", ...)\nreturn {n = 2}\n"),
        ],
        args: &["-lm", "-e", "print(m and m.n)"],
        stdin: None,
        env: &[("LUA_PATH", "./?.lua"), ("LUA_CPATH", "")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "loading\tm\nnil\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "loading\tm\t./m.lua\n1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-l mod`.
#[test]
fn require_option_separate() {
    let case = Case {
        files: &[
            ("m.lua", "print(\"loading\", ...)\nreturn {n = 1}\n"),
            ("m-v2.lua", "print(\"loading\", ...)\nreturn {n = 2}\n"),
        ],
        args: &["-l", "m", "-e", "print(m and m.n)"],
        stdin: None,
        env: &[("LUA_PATH", "./?.lua"), ("LUA_CPATH", "")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "loading\tm\nnil\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "loading\tm\t./m.lua\n1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-l g=mod` (5.4 on) stores the module in `g`; before 5.4 `g=m` is
/// the module's name.
#[test]
fn require_option_global_name() {
    let case = Case {
        files: &[
            ("m.lua", "print(\"loading\", ...)\nreturn {n = 1}\n"),
            ("m-v2.lua", "print(\"loading\", ...)\nreturn {n = 2}\n"),
        ],
        args: &["-l", "g=m", "-e", "print(g and g.n, m)"],
        stdin: None,
        env: &[("LUA_PATH", "./?.lua"), ("LUA_CPATH", "")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: module 'g=m' not found:\n\tno field package.preload['g=m']\n\tno file './g=m.lua'\nstack traceback:\n\t[C]: ?\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3"],
            stdout: "",
            stderr: "lua: module 'g=m' not found:\n\tno field package.preload['g=m']\n\tno file './g=m.lua'\nstack traceback:\n\t[C]: in function 'require'\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "loading\tm\t./m.lua\n1\tnil\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// 5.4 on leave a `-suffix` out of the global's name.
#[test]
fn require_option_version_suffix() {
    let case = Case {
        files: &[
            ("m.lua", "print(\"loading\", ...)\nreturn {n = 1}\n"),
            ("m-v2.lua", "print(\"loading\", ...)\nreturn {n = 2}\n"),
        ],
        args: &["-l", "m-v2", "-e", "print(m and m.n)"],
        stdin: None,
        env: &[("LUA_PATH", "./?.lua"), ("LUA_CPATH", "")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "loading\tm-v2\nnil\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3"],
            stdout: "loading\tm-v2\t./m-v2.lua\nnil\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "loading\tm-v2\t./m-v2.lua\n2\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// A module that is not found stops the interpreter.
#[test]
fn require_option_missing_module() {
    let case = Case {
        files: &[
            ("m.lua", "print(\"loading\", ...)\nreturn {n = 1}\n"),
            ("m-v2.lua", "print(\"loading\", ...)\nreturn {n = 2}\n"),
        ],
        args: &["-l", "nosuch", "-e", "print(1)"],
        stdin: None,
        env: &[("LUA_PATH", "./?.lua"), ("LUA_CPATH", "")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "lua: module 'nosuch' not found:\n\tno field package.preload['nosuch']\n\tno file './nosuch.lua'\nstack traceback:\n\t[C]: ?\n\t[C]: ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.2", "5.3"],
            stdout: "",
            stderr: "lua: module 'nosuch' not found:\n\tno field package.preload['nosuch']\n\tno file './nosuch.lua'\nstack traceback:\n\t[C]: in function 'require'\n\t[C]: in ?\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: module 'nosuch' not found:\n\tno field package.preload['nosuch']\n\tno file './nosuch.lua'\n\tno file ''\nstack traceback:\n\t[C]: in function 'require'\n\t[C]: in ?\n",
            status: 1,
        },
    ]);
}
