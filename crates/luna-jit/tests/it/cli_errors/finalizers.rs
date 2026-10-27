//! Finalizers as the standalone interpreter shows them: an error a 5.2/5.3
//! finalizer raises reaches `lua.c` without its message handler, finalizers
//! run when the state closes, and the 5.4+ warning for a `__gc` that cannot
//! be called.

use super::*;

/// 5.2/5.3 throw a finalizer's error with `LUA_ERRGCMM`, past the message
/// handler: `lua.c` prints it without a traceback.
#[test]
fn finalizer_error_uncaught() {
    let case = Case {
        files: &[(
            "gcmm.lua",
            "setmetatable({}, {__gc = function() error(\"boom\") end})\ncollectgarbage()\n",
        )],
        args: &["gcmm.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1", "5.4", "5.5"],
            stdout: "",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3"],
            stdout: "",
            stderr: "lua: error in __gc metamethod (gcmm.lua:1: boom)\n",
            status: 1,
        },
    ]);
}

/// An object a finalizer registers while the state closes is not finalized.
#[test]
fn registered_while_closing() {
    let case = Case {
        files: &[(
            "closereg.lua",
            "keep = setmetatable({}, {__gc = function()\n  print(\"outer finalizer\")\n  setmetatable({}, {__gc = function() print(\"inner finalizer\") end})\nend})\nprint(\"end of script\")\n",
        )],
        args: &["closereg.lua"],
        stdin: None,
        env: &[],
    };
    case.expect(&[
        Expect {
            dialects: &["5.1"],
            stdout: "end of script\n",
            stderr: "",
            status: 0,
        },
        Expect {
            dialects: &["5.2", "5.3", "5.4", "5.5"],
            stdout: "end of script\nouter finalizer\n",
            stderr: "",
            status: 0,
        },
    ]);
}

/// From 5.4 a `__gc` that is not callable gets the warning a failed call of
/// it would give.
#[test]
fn uncallable_gc_warns() {
    let case = Case {
        files: &[(
            "uncallable.lua",
            "warn(\"@on\")\nsetmetatable({}, {__gc = true})\ncollectgarbage()\nprint(\"done\")\n",
        )],
        args: &["uncallable.lua"],
        stdin: None,
        env: &[],
    };
    case.expect_dialects(&[Expect {
        dialects: &["5.4", "5.5"],
        stdout: "done\n",
        stderr: "Lua warning: error in __gc (attempt to call a boolean value (metamethod '__gc'))\n",
        status: 0,
    }]);
}

/// 5.1's `docall` and 5.2's `report` force a complete collection after an
/// error: an object already unreachable is finalized then, before the
/// ones the state's close finalizes.
#[test]
fn collection_after_error() {
    let case = Case {
        files: &[(
            "fin.lua",
            "local function mk(name)\n  if newproxy then\n    local p = newproxy(true)\n    getmetatable(p).__gc = function() io.stdout:write(name, \"\\n\") end\n    return p\n  end\n  return setmetatable({}, {__gc = function() io.stdout:write(name, \"\\n\") end})\nend\nlocal u = mk(\"unreachable\")\ng = mk(\"global\")\nu = nil\nerror(\"boom\", 0)\n",
        )],
        args: &["fin.lua"],
        stdin: None,
        env: &[],
    };
    for (d, stdout) in [
        ("5.1", "unreachable\nglobal\n"),
        ("5.2", "unreachable\nglobal\n"),
        ("5.3", "global\nunreachable\n"),
        ("5.4", "global\nunreachable\n"),
        ("5.5", "global\nunreachable\n"),
    ] {
        let o = case.run(d);
        assert_eq!(
            o.stdout,
            crate::cli_common::as_on_this_platform(stdout),
            "--lua={d}"
        );
        assert_eq!(o.status, 1, "--lua={d}");
    }
}
