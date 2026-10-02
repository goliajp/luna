//! `-l` libraries, warnings, `debug.debug`, argument order and deep `load` nesting.

use super::*;

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
