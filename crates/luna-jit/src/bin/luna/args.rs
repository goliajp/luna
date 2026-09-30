//! `lua.c`'s option parsing of each dialect and its usage message.

use luna_jit::version::LuaVersion;
use std::io::Write;

/// What `lua.c`'s `collectargs` found in the options.
#[derive(Default)]
pub(super) struct LuaArgs {
    pub(super) has_i: bool,
    pub(super) has_v: bool,
    pub(super) has_e: bool,
    /// `-E`
    pub(super) ignore_env: bool,
    /// Index of the script name in `argv`, if there is one.
    pub(super) script: Option<usize>,
}

/// `lua.c`'s `collectargs` of each dialect. `Err` holds the index of the
/// bad option (5.1 reports none, and takes it only for the usage).
pub(super) fn collectargs(v: LuaVersion, argv: &[String]) -> Result<LuaArgs, usize> {
    let mut args = LuaArgs::default();
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_bytes();
        if a.first() != Some(&b'-') {
            args.script = Some(i);
            return Ok(args);
        }
        let tail = a.len() > 2;
        match a.get(1) {
            Some(b'-') => {
                if tail {
                    return Err(i);
                }
                args.script = (i + 1 < argv.len()).then_some(i + 1);
                return Ok(args);
            }
            None => {
                args.script = Some(i);
                return Ok(args);
            }
            // 5.2 checks no characters after -E
            Some(b'E') if v == LuaVersion::Lua52 || (v >= LuaVersion::Lua53 && !tail) => {
                args.ignore_env = true;
            }
            Some(b'W') if v >= LuaVersion::Lua54 && !tail => {}
            Some(b'i' | b'v') if !tail => {
                args.has_i |= a[1] == b'i';
                args.has_v = true;
            }
            Some(o @ (b'e' | b'l')) => {
                args.has_e |= *o == b'e';
                if !tail {
                    i += 1;
                    // 5.2 on refuse another option as the argument
                    let missing = match argv.get(i) {
                        None => true,
                        Some(next) => v >= LuaVersion::Lua52 && next.starts_with('-'),
                    };
                    if missing {
                        return Err(i - 1);
                    }
                }
            }
            _ => return Err(i),
        }
        i += 1;
    }
    Ok(args)
}

/// `lua.c`'s `print_usage` of each dialect.
pub(super) fn print_usage(v: LuaVersion, progname: &str, badoption: &str) {
    let mut out = String::new();
    if v == LuaVersion::Lua51 {
        out.push_str(&format!(
            "usage: {progname} [options] [script [args]].\n\
             Available options are:\n\
             \x20 -e stat  execute string 'stat'\n\
             \x20 -l name  require library 'name'\n\
             \x20 -i       enter interactive mode after executing 'script'\n\
             \x20 -v       show version information\n\
             \x20 --       stop handling options\n\
             \x20 -        execute stdin and stop handling options\n"
        ));
    } else {
        out.push_str(&format!("{progname}: "));
        if matches!(badoption.as_bytes().get(1), Some(b'e' | b'l')) {
            out.push_str(&format!("'{badoption}' needs argument\n"));
        } else {
            out.push_str(&format!("unrecognized option '{badoption}'\n"));
        }
        out.push_str(&format!("usage: {progname} [options] [script [args]]\n"));
        out.push_str("Available options are:\n");
        out.push_str(match v {
            LuaVersion::Lua52 => {
                "  -e stat  execute string 'stat'\n\
                 \x20 -i       enter interactive mode after executing 'script'\n\
                 \x20 -l name  require library 'name'\n\
                 \x20 -v       show version information\n\
                 \x20 -E       ignore environment variables\n\
                 \x20 --       stop handling options\n\
                 \x20 -        stop handling options and execute stdin\n"
            }
            LuaVersion::Lua53 => {
                "  -e stat  execute string 'stat'\n\
                 \x20 -i       enter interactive mode after executing 'script'\n\
                 \x20 -l name  require library 'name' into global 'name'\n\
                 \x20 -v       show version information\n\
                 \x20 -E       ignore environment variables\n\
                 \x20 --       stop handling options\n\
                 \x20 -        stop handling options and execute stdin\n"
            }
            _ => {
                "  -e stat   execute string 'stat'\n\
                 \x20 -i        enter interactive mode after executing 'script'\n\
                 \x20 -l mod    require library 'mod' into global 'mod'\n\
                 \x20 -l g=mod  require library 'mod' into global 'g'\n\
                 \x20 -v        show version information\n\
                 \x20 -E        ignore environment variables\n\
                 \x20 -W        turn warnings on\n\
                 \x20 --        stop handling options\n\
                 \x20 -         stop handling options and execute stdin\n"
            }
        });
    }
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(out.as_bytes()); // nowhere left to report a failed write
}
