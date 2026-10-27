//! `LUA_DIRSEP`, `\` on Windows and `/` elsewhere, as PUC built for the
//! platform has it: the first line of `package.config`, and what
//! `package.searchpath` (unless told otherwise) and `require` turn a
//! module name's dots into.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SEP: &str = if cfg!(windows) { "\\" } else { "/" };

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn eval_str(v: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(v);
    match vm.eval(src).map(|r| r.first().copied()) {
        Ok(Some(Value::Str(s))) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{v:?}: {src} gave {other:?}"),
    }
}

/// A directory holding `a/b.lua`, and the prefix of a template in it.
fn module_dir(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("luna-dirsep-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(dir.join("a")).expect("create the module dir");
    std::fs::write(dir.join("a").join("b.lua"), "return 'found a.b'").expect("write a/b.lua");
    format!("{}/", dir.display())
}

#[test]
fn config_starts_with_the_dirsep() {
    for v in ALL {
        let end = if v == LuaVersion::Lua51 { "" } else { "\n" };
        let want = format!("{SEP}\n;\n?\n!\n-{end}");
        assert_eq!(eval_str(v, "return package.config"), want, "{v:?}");
    }
}

#[test]
fn searchpath_turns_dots_into_the_dirsep() {
    let pre = module_dir("searchpath");
    for v in &ALL[1..] {
        let found = eval_str(
            *v,
            &format!("return package.searchpath('a.b', [==[{pre}?.lua]==])"),
        );
        assert_eq!(found, format!("{pre}a{SEP}b.lua"), "{v:?}");
        let msg = eval_str(
            *v,
            &format!("local _, e = package.searchpath('x.y', [==[{pre}?.lua]==]) return e"),
        );
        let want = format!("no file '{pre}x{SEP}y.lua'");
        assert!(msg.contains(&want), "{v:?}: {msg:?} lacks {want:?}");
        // an explicit `rep` still wins
        let rep = eval_str(
            *v,
            &format!(
                "local _, e = package.searchpath('x.y', [==[{pre}?.lua]==], '.', '+') return e"
            ),
        );
        assert!(rep.contains(&format!("{pre}x+y.lua")), "{v:?}: {rep:?}");
    }
}

#[test]
fn require_turns_dots_into_the_dirsep() {
    let pre = module_dir("require");
    for v in ALL {
        let src = format!("package.path = [==[{pre}?.lua]==] return require 'a.b'");
        assert_eq!(eval_str(v, &src), "found a.b", "{v:?}");
        let src = format!(
            "package.path = [==[{pre}?.lua]==] local _, e = pcall(require, 'x.y') return e"
        );
        let msg = eval_str(v, &src);
        let want = format!("no file '{pre}x{SEP}y.lua'");
        assert!(msg.contains(&want), "{v:?}: {msg:?} lacks {want:?}");
    }
}
