//! The `luna` CLI takes `lua.c`'s options as each dialect's `lua.c` does:
//! `-v` alone and with the other options, `--` and `-`, and the one-letter
//! options that must stand alone (`-E`, `-W`).
//!
//! Every expectation was recorded from the stock PUC interpreters — Lua
//! 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1, each built with `make linux` on
//! Linux x86_64 — run as `lua <args>` from a directory holding the case's
//! files; `cli_common` says how luna's output is compared.

use crate::cli_common::{Case, Expect};

/// `-v` alone prints the version line and reads no program from stdin
/// (5.1 prints it on stderr).
#[test]
fn version() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-v"],
        stdin: Some("print(7)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-v` and then the script, which still runs.
#[test]
fn version_then_script() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-v", "s.lua", "a"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "s\ta\n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\ns\ta\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-v` with `-e`: the version line comes first.
#[test]
fn version_with_inline_chunk() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-v", "-e", "print(1)"],
        stdin: Some("print(7)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "1\n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-v -`: the version line, then the program on stdin.
#[test]
fn version_then_stdin() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-v", "-"],
        stdin: Some("print(7)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "7\n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n7\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-v` given twice prints the line once.
#[test]
fn version_twice() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-v", "-v"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-vx` is not `-v`.
#[test]
fn version_with_tail() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-vx"],
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
            stderr: "lua: unrecognized option '-vx'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: unrecognized option '-vx'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: unrecognized option '-vx'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat   execute string 'stat'\n  -i        enter interactive mode after executing 'script'\n  -l mod    require library 'mod' into global 'mod'\n  -l g=mod  require library 'mod' into global 'g'\n  -v        show version information\n  -E        ignore environment variables\n  -W        turn warnings on\n  --        stop handling options\n  -         stop handling options and execute stdin\n",
            status: 1,
        },
    ]);
}

/// `--` stops the options; the script follows.
#[test]
fn double_dash_then_script() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["--", "s.lua", "a"],
        stdin: None,
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "s\ta\n",
        stderr: "",
        status: 0,
    }]);
}

/// `--` with nothing after it: no script, so stdin is the program.
#[test]
fn double_dash_alone() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["--"],
        stdin: Some("print(7)\n"),
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "7\n",
        stderr: "",
        status: 0,
    }]);
}

/// `-` runs stdin as the script, with the arguments after it.
#[test]
fn dash_is_stdin() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-", "a", "b"],
        stdin: Some("print(...)\n"),
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "a\tb\n",
        stderr: "",
        status: 0,
    }]);
}

/// After `--`, `-` names a file.
#[test]
fn dash_after_double_dash_is_a_file() {
    let case = Case {
        files: &[
            ("s.lua", "print(\"s\", ...)\n"),
            ("-", "print(\"the file named -\")\n"),
        ],
        args: &["--", "-"],
        stdin: Some("print(7)\n"),
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "the file named -\n",
        stderr: "",
        status: 0,
    }]);
}

/// Whatever follows `-` goes to the script.
#[test]
fn options_after_dash_are_arguments() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-", "--", "-v"],
        stdin: Some("print(...)\n"),
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "--\t-v\n",
        stderr: "",
        status: 0,
    }]);
}

/// After `--`, a name that looks like an option is the script.
#[test]
fn option_named_script_after_double_dash() {
    let case = Case {
        files: &[("-i", "print(\"the file named -i\", ...)\n")],
        args: &["-v", "--", "-i", "x"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "the file named -i\tx\n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\nthe file named -i\tx\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// 5.2 takes anything after `-E`; 5.3 on refuse it.
#[test]
fn ignore_environment_with_tail() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-Ex", "-e", "print(1)"],
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
            stdout: "1\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: unrecognized option '-Ex'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: unrecognized option '-Ex'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat   execute string 'stat'\n  -i        enter interactive mode after executing 'script'\n  -l mod    require library 'mod' into global 'mod'\n  -l g=mod  require library 'mod' into global 'g'\n  -v        show version information\n  -E        ignore environment variables\n  -W        turn warnings on\n  --        stop handling options\n  -         stop handling options and execute stdin\n",
            status: 1,
        },
    ]);
}

/// `-Wx` is not `-W`.
#[test]
fn warnings_with_tail() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-Wx"],
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
            stderr: "lua: unrecognized option '-Wx'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "",
            stderr: "lua: unrecognized option '-Wx'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat  execute string 'stat'\n  -i       enter interactive mode after executing 'script'\n  -l name  require library 'name' into global 'name'\n  -v       show version information\n  -E       ignore environment variables\n  --       stop handling options\n  -        stop handling options and execute stdin\n",
            status: 1,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "",
            stderr: "lua: unrecognized option '-Wx'\nusage: lua [options] [script [args]]\nAvailable options are:\n  -e stat   execute string 'stat'\n  -i        enter interactive mode after executing 'script'\n  -l mod    require library 'mod' into global 'mod'\n  -l g=mod  require library 'mod' into global 'g'\n  -v        show version information\n  -E        ignore environment variables\n  -W        turn warnings on\n  --        stop handling options\n  -         stop handling options and execute stdin\n",
            status: 1,
        },
    ]);
}

/// `-W` (5.4 on) turns warnings on before `-e` runs.
#[test]
fn warnings_on_for_inline_chunk() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-W", "-e", "warn('w')"],
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
            stdout: "",
            stderr: "Lua warning: w\n",
            status: 0,
        },
    ]);
}
