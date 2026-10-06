//! The stack a `__tostring` sees when the REPL prints a chunk's results:
//! lua.c's `l_print` calls `print` from inside `pmain`, its C function, so
//! below `print` (and in 5.1 to 5.3 `tostring`) there is one more C level,
//! which `debug.traceback` and `debug.getinfo` find. Recorded from the
//! stock PUC 5.1.5 to 5.5.1 interpreters as `lua -i` reading this stdin.

use crate::cli_common::{as_on_this_platform, run, workdir};

const STDIN: &str = r#"mt = {__tostring = function() return debug.traceback("tb", 1) end}
t = setmetatable({}, mt)
return t
lv = function() local s = {} for l = 1, 6 do local i = debug.getinfo(l, "Sn") s[#s + 1] = i and (i.what .. ":" .. tostring(i.name) .. ":" .. i.short_src) or "nil" end return table.concat(s, ",") end
u = setmetatable({}, {__tostring = lv})
return u
return 1, u
print(u)
"#;

const OLD: &str = "> > > tb\nstack traceback:\n\tstdin:1: in function <stdin:1>\n\t[C]: in function 'tostring'\n\t[C]: in function 'print'\n\t[C]: in ?\n> > > Lua:nil:stdin,C:nil:[C],C:nil:[C],C:nil:[C],nil,nil\n> 1\tLua:nil:stdin,C:nil:[C],C:nil:[C],C:nil:[C],nil,nil\n> Lua:nil:stdin,C:nil:[C],C:print:[C],main:nil:stdin,C:nil:[C],nil\n> \n";

const NEW: &str = "> > > tb\nstack traceback:\n\tstdin:1: in function <stdin:1>\n\t[C]: in function 'print'\n\t[C]: in ?\n> > > Lua:nil:stdin,C:nil:[C],C:nil:[C],nil,nil,nil\n> 1\tLua:nil:stdin,C:nil:[C],C:nil:[C],nil,nil,nil\n> Lua:nil:stdin,C:print:[C],main:nil:stdin,C:nil:[C],nil,nil\n> \n";

#[test]
fn tostring_in_printed_results_sees_pmain() {
    let dir = workdir(&[]);
    let v51 = "> > > tb\nstack traceback:\n\tstdin:1: in function <stdin:1>\n\t[C]: ?\n\t[C]: ?\n\t[C]: ?\n> > > Lua:nil:stdin,C:nil:[C],C:nil:[C],C:nil:[C],nil,nil\n> 1\tLua:nil:stdin,C:nil:[C],C:nil:[C],C:nil:[C],nil,nil\n> Lua:nil:stdin,C:nil:[C],C:print:[C],main:nil:stdin,C:nil:[C],nil\n> \n";
    let with_version = |s: &str| format!("<version>\n{s}");
    let cases = [
        ("5.1", v51.to_string(), "<version>\n".to_string()),
        ("5.2", with_version(OLD), String::new()),
        ("5.3", with_version(OLD), String::new()),
        ("5.4", with_version(NEW), String::new()),
        ("5.5", with_version(NEW), String::new()),
    ];
    for (d, stdout, stderr) in cases {
        let out = run(d, &dir, &["-i"], Some(STDIN), &[]);
        // 5.2 names a library function by whichever of its names it meets
        // first in hash order ('tostring' or '_G.tostring')
        let got = out.stdout.replace("'_G.", "'");
        assert_eq!(got, as_on_this_platform(&stdout), "stdout, --lua={d}");
        assert_eq!(
            out.stderr,
            as_on_this_platform(&stderr),
            "stderr, --lua={d}"
        );
        assert_eq!(out.status, 0);
    }
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
}
