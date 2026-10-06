//! `errno/setters.lua`: after each call that can leave something in the C
//! library's `errno` (a numeral out of range, a math function out of its
//! domain, a failed open), each failure that reports `errno`. Against what
//! PUC 5.1.5 to 5.5.1 printed on x86_64 Linux: with glibc every such
//! failure sets `errno` itself, so nothing earlier shows through.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("../errno/setters.lua");
const PUC: [&str; 5] = [
    include_str!("../errno/setters.5.1.txt"),
    include_str!("../errno/setters.5.2.txt"),
    include_str!("../errno/setters.5.3.txt"),
    include_str!("../errno/setters.5.4.txt"),
    include_str!("../errno/setters.5.5.txt"),
];

#[test]
fn failures_report_errno_as_puc_on_glibc() {
    let dialects = [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ];
    for (v, want) in dialects.into_iter().zip(PUC) {
        let dir = std::env::temp_dir().join(format!("luna-errno-{}-{v:?}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the work dir");
        let src = format!(
            "DIR = [==[{}/]==]
             local OUT = {{}}
             print = function(...)
               local t = {{}}
               for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
               OUT[#OUT + 1] = table.concat(t, '\\t')
             end
             do {SCRIPT}
             end
             return table.concat(OUT, '\\n') .. '\\n'",
            dir.display()
        );
        let mut vm = Vm::new(v);
        let got = match vm.eval(&src) {
            Ok(r) => match r.first() {
                Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                other => panic!("{v:?}: the script returned {other:?}"),
            },
            Err(e) => panic!("{v:?}: {}", vm.error_text(&e)),
        };
        std::fs::remove_dir_all(&dir).expect("remove the work dir");
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
