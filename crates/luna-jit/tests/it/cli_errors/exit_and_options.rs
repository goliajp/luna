//! `os.exit` statuses and the usage message for bad options.

use super::*;

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
