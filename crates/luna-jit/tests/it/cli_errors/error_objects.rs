//! Non-string error objects and `__tostring` on the error value.

use super::*;

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
