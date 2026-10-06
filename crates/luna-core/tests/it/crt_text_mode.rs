//! Files in the MSVC C library's text mode (`Vm::set_crt_text_mode`), on
//! every platform. `crt_text/files.lua` writes and reads files with and
//! without `b` and prints what it sees; the expectations are what PUC 5.1.5
//! to 5.5.0 built with MSVC 19.51 (`cl /O2 /MD`) printed for it on
//! windows-latest (5.2 to 5.5 print the same). They include the positions
//! `seek` reports, which for a file whose lines end in a bare `\n` are the
//! library's miscalculation, and its failure where that comes out negative.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("../crt_text/files.lua");
const PUC_51: &str = include_str!("../crt_text/files.5.1.txt");
const PUC: &str = include_str!("../crt_text/files.txt");

/// `crt_text/more.lua`: `setvbuf` sizes and the positions after them, what
/// a failed number read leaves (5.1 and 5.2 read with the library's
/// `fscanf`), `ungetc` at the start of a buffer, and a write right after a
/// read, which the library refuses and which leaves stale buffer bytes in
/// the file. Runs of NUL bytes are written `<\0*N>` on both sides.
const MORE: &str = include_str!("../crt_text/more.lua");
const MORE_PUC: [&str; 5] = [
    include_str!("../crt_text/more.5.1.txt"),
    include_str!("../crt_text/more.5.2.txt"),
    include_str!("../crt_text/more.5.3.txt"),
    include_str!("../crt_text/more.5.4.txt"),
    include_str!("../crt_text/more.5.5.txt"),
];

fn run(v: LuaVersion, tag: &str) -> String {
    run_script(v, tag, SCRIPT)
}

fn run_script(v: LuaVersion, tag: &str, script: &str) -> String {
    let dir = std::env::temp_dir().join(format!("luna-crt-text-{}-{tag}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("remove a stale work dir");
    }
    std::fs::create_dir_all(&dir).expect("create the work dir");
    let prefix = format!("{}/", dir.display()).replace('\\', "/");
    let src = format!(
        "DIR = [==[{prefix}]==]
         local OUT = {{}}
         print = function(...)
           local t = {{}}
           for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
           OUT[#OUT + 1] = table.concat(t, '\\t')
         end
         do {script}
         end
         return table.concat(OUT, '\\n') .. '\\n'"
    );
    let mut vm = Vm::new(v);
    vm.set_crt_text_mode(true);
    let out = match vm.eval(&src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{v:?}: the script returned {other:?}"),
        },
        Err(e) => panic!("{v:?}: {}", vm.error_text(&e)),
    };
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
    out
}

#[test]
fn files_read_and_write_as_in_puc_built_with_msvc() {
    for (v, tag, want) in [
        (LuaVersion::Lua51, "51", PUC_51),
        (LuaVersion::Lua52, "52", PUC),
        (LuaVersion::Lua53, "53", PUC),
        (LuaVersion::Lua54, "54", PUC),
        (LuaVersion::Lua55, "55", PUC),
    ] {
        // a Windows checkout may give the recording CRLF line endings
        let want = want.replace("\r\n", "\n");
        let got = run(v, tag);
        for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
            assert_eq!(g, w, "{v:?}, line {}", i + 1);
        }
        assert_eq!(
            got.lines().count(),
            want.lines().count(),
            "{v:?}: line count"
        );
    }
}

fn compress_nuls(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("\\0") {
        out.push_str(&rest[..i]);
        let mut n = 0;
        let mut r = &rest[i..];
        while let Some(t) = r.strip_prefix("\\0") {
            n += 1;
            r = t;
        }
        if n >= 8 {
            out.push_str(&format!("<\\0*{n}>"));
        } else {
            out.push_str(&"\\0".repeat(n));
        }
        rest = r;
    }
    out.push_str(rest);
    out
}

#[test]
fn more_stream_behaviour_as_in_puc_built_with_msvc() {
    let dialects = [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ];
    for (v, want) in dialects.into_iter().zip(MORE_PUC) {
        let want = want.replace("\r\n", "\n");
        let got = compress_nuls(&run_script(v, &format!("more{v:?}"), MORE));
        for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
            assert_eq!(g, w, "{v:?}, line {}", i + 1);
        }
        assert_eq!(
            got.lines().count(),
            want.lines().count(),
            "{v:?}: line count"
        );
    }
}

/// Without the setting (the default) nothing is translated.
#[test]
fn files_are_binary_by_default() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let path = std::env::temp_dir().join(format!("luna-crt-bin-{}", std::process::id()));
    let p = path.display().to_string().replace('\\', "/");
    let r = vm
        .eval(&format!(
            "local f = assert(io.open([==[{p}]==], 'w')) f:write('a\\nb\\r\\n') f:close()
             f = assert(io.open([==[{p}]==], 'rb')) local s = f:read('a') f:close()
             return s"
        ))
        .expect("runs");
    std::fs::remove_file(&path).expect("remove the file");
    match r.first() {
        Some(Value::Str(s)) => assert_eq!(s.as_bytes(), b"a\nb\r\n"),
        other => panic!("{other:?}"),
    }
}
