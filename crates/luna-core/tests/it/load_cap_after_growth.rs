//! A text load whose main function's code array grows to and past a power
//! of two, then a memory cap at the count the load leaves: the cap is held
//! at the first instruction after the load, which collects in full when
//! the count is over it. The counts are PUC's, recorded from an embedder
//! like the one in `open_running` (hash seed 1, no coincidences of
//! addresses).

use luna_core::__private::VmCApiLoad;
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// The chunk, with the loaded source `n` statements long and written in
/// it, so that nothing is made before the load: it reads the count the
/// load left, collects, and reads it again.
fn chunk(n: u32) -> String {
    let body = "x = x + 1\n".repeat(n as usize);
    format!(
        "local load = loadstring or load \
         local f = assert(load([[local x = 0\n{body}]])) \
         local c = collectgarbage(\"count\") * 1024 \
         collectgarbage() \
         return string.format(\"%d %d\", c, collectgarbage(\"count\") * 1024)"
    )
}

/// What the chunk returns run by an embedder with the collector running
/// and the libraries open, under `cap`; or the error it raised.
fn run(v: LuaVersion, n: u32, cap: Option<usize>) -> Result<(u64, u64), String> {
    let mut vm = Vm::new_minimal_with_hash_seed(v, 1);
    vm.open_all_libs().expect("the libraries open");
    let src = chunk(n);
    let Ok((f, _)) = vm.host_load_chunk(src.as_bytes(), src.as_bytes()) else {
        panic!("{v:?}: the chunk compiles");
    };
    vm.set_memory_cap(cap);
    let r = vm
        .call_value(Value::Closure(f), &[])
        .map_err(|e| vm.error_display(&e))?;
    let Some(Value::Str(s)) = r.first() else {
        panic!("{v:?}: a string");
    };
    let s = String::from_utf8_lossy(s.as_bytes()).into_owned();
    let (a, b) = s.split_once(' ').expect("two counts");
    Ok((a.parse().expect("a count"), b.parse().expect("a count")))
}

/// PUC's counts after the load and after a full collection, by `n`.
const RECORDED: [(LuaVersion, [(u32, u64, u64); 6]); 5] = [
    (
        LuaVersion::Lua51,
        [
            (255, 35860, 34707),
            (256, 35888, 34735),
            (511, 43003, 41875),
            (512, 43031, 41903),
            (1023, 57299, 56211),
            (1024, 57327, 56239),
        ],
    ),
    (
        LuaVersion::Lua52,
        [
            (255, 33517, 33091),
            (256, 33555, 33129),
            (511, 43220, 42819),
            (512, 43258, 42857),
            (1023, 62676, 62275),
            (1024, 62714, 62313),
        ],
    ),
    (
        LuaVersion::Lua53,
        [
            (255, 34176, 33968),
            (256, 34214, 34006),
            (511, 43904, 43696),
            (512, 43942, 43734),
            (1023, 63360, 63152),
            (1024, 63398, 63190),
        ],
    ),
    (
        LuaVersion::Lua54,
        [
            (255, 32556, 32012),
            (256, 32596, 32052),
            (511, 42388, 42284),
            (512, 42428, 42324),
            (1023, 63372, 62828),
            (1024, 63412, 62868),
        ],
    ),
    (
        LuaVersion::Lua55,
        [
            (255, 31465, 30368),
            (256, 31505, 30408),
            (511, 41737, 40640),
            (512, 41777, 40680),
            (1023, 62256, 61184),
            (1024, 62296, 61224),
        ],
    ),
];

// the package paths in the count are another directory's on Windows, and
// with `gc-stress` every checkpoint collects
#[test]
#[cfg(all(target_pointer_width = "64", not(windows), not(feature = "gc-stress")))]
fn a_cap_after_the_code_grows_is_met_at_the_count_puc_has() {
    for (v, cases) in RECORDED {
        for (n, after, collected) in cases {
            let at = |cap: u64| run(v, n, Some(cap as usize)).map(|(c, _)| c);
            assert_eq!(run(v, n, None), Ok((after, collected)), "{v:?} n={n}");
            // at the count the load left: nothing happens
            assert_eq!(at(after), Ok(after), "{v:?} n={n}");
            // one byte under it: the first instruction after the load
            // collects in full before the chunk reads the count
            let read = at(after - 1).expect("under the cap once collected");
            assert!(read < after, "{v:?} n={n}: {read}");
        }
    }
}
