//! The REPL of the `luna` CLI, as each dialect's `lua.c` runs it when
//! built without readline: prompts on stdout, lines read from stdin,
//! results through `print`, errors with their traceback.
//!
//! Every expectation was recorded from the stock PUC interpreters — Lua
//! 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1, each built with `make linux` on
//! Linux x86_64 — run as `lua <args>` from a directory holding the case's
//! files; `cli_common` says how luna's output is compared.

mod cli_common;

use cli_common::{Case, Expect};

/// A session: expressions (5.3 on), `=expr` (through 5.4), continuation
/// lines, errors with their traceback, 5.5's warning about `local`, and an
/// unfinished string that takes the next line with it (5.3 on).
#[test]
fn transcript() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some(
            "1+1\nx = 5\nx\n=x\nprint(\"p\")\nif x then\nprint(\"in\")\nend\nerror(\"boom\")\nerror({})\nlocal a = 1\nnosuch()\n\"abc\n_PROMPT=\"P$ \"\n3\n_PROMPT2=7\nfor i=1,2 do\nprint(i) end\nreturn 9, nil\n",
        ),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > >> > p\n> >> >> in\n> > > > > >> > > > 71\n2\n> 9\tnil\n> \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '1'\nstdin:1: boom\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: ?\n(error object is not a string)\nstdin:1: attempt to call global 'nosuch' (a nil value)\nstack traceback:\n\tstdin:1: in main chunk\n\t[C]: ?\nstdin:1: unfinished string near '\"abc'\nstdin:1: unexpected symbol near '3'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > >> > p\n> >> >> in\n> > > > > >> > > > 71\n2\n> 9\tnil\n> \n",
            stderr: "stdin:1: unexpected symbol near '1'\nstdin:1: boom\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n(no error message)\nstdin:1: attempt to call global 'nosuch' (a nil value)\nstack traceback:\n\tstdin:1: in main chunk\n\t[C]: in ?\nstdin:1: unfinished string near '\"abc'\nstdin:1: unexpected symbol near '3'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> 2\n> > 5\n> 5\n> p\n> >> >> in\n> > > > > >> > 3\n> > 71\n2\n> 9\tnil\n> \n",
            stderr: "stdin:1: boom\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n(error object is a table value)\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\nstdin:1: attempt to call a nil value (global 'nosuch')\nstack traceback:\n\tstdin:1: in main chunk\n\t[C]: in ?\nstdin:1: unfinished string near '\"abc'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> 2\n> > 5\n> > p\n> >> >> in\n> > > > > >> > 3\n> > 71\n2\n> 9\tnil\n> \n",
            stderr: "stdin:1: unexpected symbol near '='\nstdin:1: boom\nstack traceback:\n\t[C]: in global 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n(error object is a table value)\nstack traceback:\n\t[C]: in global 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\nwarning: locals do not survive across lines in interactive mode\nstdin:1: attempt to call a nil value (global 'nosuch')\nstack traceback:\n\tstdin:1: in main chunk\n\t[C]: in ?\nstdin:1: unfinished string near '\"abc'\n",
            status: 0,
        },
    ]);
}

/// `-i` enters the REPL after the script ran; the version line first.
#[test]
fn after_script() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i", "s.lua", "a"],
        stdin: Some("print(arg[1], #arg)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "s\ta\n> a\t1\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\ns\ta\n> a\t1\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-e` runs before the REPL.
#[test]
fn after_inline_chunk() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-e", "x=1", "-i"],
        stdin: Some("x\n=x\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> >> > \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> >> > \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> 1\n> 1\n> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> 1\n> > \n",
            stderr: "stdin:1: unexpected symbol near '='\n",
            status: 0,
        },
    ]);
}

/// `-i -v` prints the version line once.
#[test]
fn with_version() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i", "-v"],
        stdin: Some("print(1)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> 1\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n> 1\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `-i -`: stdin is the script, and the REPL then finds no input.
#[test]
fn after_stdin_script() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i", "-"],
        stdin: Some("print(7)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "7\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n7\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// After `-`, `-i` is the script's argument.
#[test]
fn dash_then_i() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-", "-i"],
        stdin: Some("print(...)\n"),
        env: &[],
    };
    case.expect(&[Expect {
        dialects: &["5.1", "5.2", "5.3", "5.4", "5.5"],
        stdout: "-i\n",
        stderr: "",
        status: 0,
    }]);
}

/// `io.read` in the REPL reads the next line of the same stdin.
#[test]
fn shares_stdin() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("x = io.read()\nhello\nprint(x)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > hello\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n> > hello\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// The prompt and `io.write`'s output keep their order.
#[test]
fn output_order() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("do io.write('w') end\ndo io.write('v\\n') end\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> w> v\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n> w> v\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// A line longer than the 512-byte buffer is read in pieces.
#[test]
fn long_line() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some(
            "print('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')\nprint(2)\n",
        ),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> >> > 2\n> \n",
            stderr: "<version>\nstdin:1: unfinished string near ''aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n> >> > 2\n> \n",
            stderr: "stdin:1: unfinished string near ''aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\n",
            status: 0,
        },
    ]);
}

/// `_PROMPT` / `_PROMPT2`: a string or number; 5.4 on convert anything else
/// with `tostring`'s rules.
#[test]
fn prompts() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some(
            "_PROMPT=\"P$ \"\n3\n_PROMPT2=7\nfor i=1,2 do\nprint(i) end\n_PROMPT=true\n4\n_PROMPT=setmetatable({},{__tostring=function() return \"T:\" end})\n5\n",
        ),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> P$ P$ P$ 71\n2\nP$ > > > > \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '3'\nstdin:1: unexpected symbol near '4'\nstdin:1: unexpected symbol near '5'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> P$ P$ P$ 71\n2\nP$ > > > > \n",
            stderr: "stdin:1: unexpected symbol near '3'\nstdin:1: unexpected symbol near '4'\nstdin:1: unexpected symbol near '5'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "<version>\n> P$ 3\nP$ P$ 71\n2\nP$ > 4\n> > 5\n> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "<version>\n> P$ 3\nP$ P$ 71\n2\nP$ true4\ntrueT:5\nT:\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// `=` and `return` lines, `local` (5.5 warns when a line starts with it)
/// and a statement over two lines.
#[test]
fn first_line_forms() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("=1\nreturn 1\n local b\nlocal\nlocalx = 1\n\nx=\n1\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> 1\n> 1\n> > >> > > >> > \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4"],
            stdout: "<version>\n> 1\n> 1\n> > >> > > >> > \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> > 1\n> > >> > > >> > \n",
            stderr: "stdin:1: unexpected symbol near '='\nwarning: locals do not survive across lines in interactive mode\nwarning: locals do not survive across lines in interactive mode\n",
            status: 0,
        },
    ]);
}

/// `os.exit` in the REPL.
#[test]
fn os_exit() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("os.exit(3)\nprint(1)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> ",
            stderr: "<version>\n",
            status: 3,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n> ",
            stderr: "",
            status: 3,
        },
    ]);
}

/// `LUA_INIT` runs before the REPL.
#[test]
fn lua_init() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("print(1)\n"),
        env: &[("LUA_INIT", "print('init')")],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "init\n> 1\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\ninit\n> 1\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}
