//! Syntax errors, `-e` chunks and scripts read from stdin.

use super::*;

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
