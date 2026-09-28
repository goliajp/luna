//! The REPL of the `luna` CLI at its edges, as each dialect's `lua.c`
//! handles them: a `print` that fails or is missing, input that ends
//! inside a statement, a prompt that cannot be converted, error objects
//! that are not strings, NUL bytes.
//!
//! Every expectation was recorded from the stock PUC interpreters — Lua
//! 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1, each built with `make linux` on
//! Linux x86_64 — run as `lua <args>` from a directory holding the case's
//! files; `cli_common` says how luna's output is compared.

use crate::cli_common::{Case, Expect};

/// Results with no global `print`.
#[test]
fn print_missing() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("print = nil\n1\n=1\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > > \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '1'\nerror calling 'print' (attempt to call a nil value)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > > \n",
            stderr: "stdin:1: unexpected symbol near '1'\nerror calling 'print' (attempt to call a nil value)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> > > > \n",
            stderr: "error calling 'print' (attempt to call a nil value)\nerror calling 'print' (attempt to call a nil value)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> > > > \n",
            stderr: "error calling 'print' (attempt to call a nil value)\nstdin:1: unexpected symbol near '='\n",
            status: 0,
        },
    ]);
}

/// A `print` that raises a table.
#[test]
fn print_raises_table() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("print = function() error({}) end\n1\n=1\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > > \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '1'\nerror calling 'print' ((null))\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > > \n",
            stderr: "stdin:1: unexpected symbol near '1'\nerror calling 'print' ((null))\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> > > > \n",
            stderr: "error calling 'print' ((null))\nerror calling 'print' ((null))\n",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> > > > \n",
            stderr: "error calling 'print' ((null))\nstdin:1: unexpected symbol near '='\n",
            status: 0,
        },
    ]);
}

/// A `print` that raises a string.
#[test]
fn print_raises_string() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("print = function() error('pe') end\n1\n=1\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > > \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '1'\nerror calling 'print' (stdin:1: pe)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > > \n",
            stderr: "stdin:1: unexpected symbol near '1'\nerror calling 'print' (stdin:1: pe)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> > > > \n",
            stderr: "error calling 'print' (stdin:1: pe)\nerror calling 'print' (stdin:1: pe)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> > > > \n",
            stderr: "error calling 'print' (stdin:1: pe)\nstdin:1: unexpected symbol near '='\n",
            status: 0,
        },
    ]);
}

/// Input ends inside a statement: 5.1 and 5.2 drop it, 5.4 on report it,
/// and 5.3 reports the `_PROMPT2` it last fetched.
#[test]
fn eof_mid_statement() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("if x then\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> >> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> >> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "<version>\n> >> > \n",
            stderr: "(null)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "<version>\n> >> > \n",
            stderr: "stdin:1: 'end' expected near <eof>\n",
            status: 0,
        },
    ]);
}

/// As above, with `_PROMPT2` set.
#[test]
fn eof_mid_statement_prompt2() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("_PROMPT2 = 'P2'\nif x then\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > P2\n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > P2\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "<version>\n> > P2> \n",
            stderr: "P2\n",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "<version>\n> > P2> \n",
            stderr: "stdin:1: 'end' expected near <eof>\n",
            status: 0,
        },
    ]);
}

/// Input ends inside a long string.
#[test]
fn eof_mid_string() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("x = [[\nab\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> >> >> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> >> >> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "<version>\n> >> >> > \n",
            stderr: "(null)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "<version>\n> >> >> > \n",
            stderr: "stdin:2: unfinished long string (starting at line 1) near <eof>\n",
            status: 0,
        },
    ]);
}

/// 5.4 on: a `_PROMPT` whose `__tostring` raises ends the interpreter.
#[test]
fn prompt_raises() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("_PROMPT=setmetatable({},{__tostring=function() error('bad') end})\n1\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '1'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > \n",
            stderr: "stdin:1: unexpected symbol near '1'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "<version>\n> > 1\n> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "<version>\n> ",
            stderr: "stdin:1: bad\n",
            status: 1,
        },
    ]);
}

/// 5.4 on: a `__tostring` that returns no string.
#[test]
fn prompt_tostring_not_string() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("_PROMPT=setmetatable({},{__tostring=function() return 1 end})\n1\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > \n",
            stderr: "<version>\nstdin:1: unexpected symbol near '1'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > \n",
            stderr: "stdin:1: unexpected symbol near '1'\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3"],
            stdout: "<version>\n> > 1\n> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.4", "5.5"],
            stdout: "<version>\n> 11\n1\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// Error objects that are not strings.
#[test]
fn error_objects() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some(
            "error(nil)\nerror(12)\nerror(setmetatable({}, {__tostring=function() return 'ts' end}))\n",
        ),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> > > > \n",
            stderr: "<version>\nstdin:1: 12\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: ?\n(error object is not a string)\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> > > > \n",
            stderr: "stdin:1: 12\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\nts\n",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> > > > \n",
            stderr: "(error object is a nil value)\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n12\nstack traceback:\n\t[C]: in function 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\nts\n",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> > > > \n",
            stderr: "(error object is a nil value)\nstack traceback:\n\t[C]: in global 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\n12\nstack traceback:\n\t[C]: in global 'error'\n\tstdin:1: in main chunk\n\t[C]: in ?\nts\n",
            status: 0,
        },
    ]);
}

/// A line is taken up to its first NUL.
#[test]
fn nul_in_line() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("print(1)\0 ignored\nprint(2)\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> 1\n> 2\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "<version>\n> 1\n> 2\n> \n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// A function defined over several lines, then called.
#[test]
fn function_over_lines() {
    let case = Case {
        files: &[("s.lua", "print(\"s\", ...)\n")],
        args: &["-i"],
        stdin: Some("function f()\nreturn 1\nend\nf()\n=f()\n"),
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "> >> >> > > 1\n> \n",
            stderr: "<version>\n",
            status: 0,
        },
        Expect {
            dialects: &["5.2"],
            stdout: "<version>\n> >> >> > > 1\n> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.3", "5.4"],
            stdout: "<version>\n> >> >> > 1\n> 1\n> \n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.5"],
            stdout: "<version>\n> >> >> > 1\n> > \n",
            stderr: "stdin:1: unexpected symbol near '='\n",
            status: 0,
        },
    ]);
}
