//! A 5.1 or 5.2 load with a collection at every check: each function's
//! table of names is dropped when it ends, the collection frees what only
//! that table held, and the next function's scanner makes its names
//! again, from the strings the load's prototypes keep. A string taken that
//! was freed shows as a freed block written or read (or, under a
//! sanitizer, as a use after free on the ordinary Vm).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;

use super::refusing_alloc::run;

/// A chunk of `n` functions that share their local and field names, run
/// with the pause at zero while it loads, so that a step is due at every
/// check.
fn source(n: usize) -> String {
    let mut chunk = String::from("local out = {}\n");
    for i in 0..n {
        chunk.push_str(&format!(
            "local function f{i}(alpha, beta)\n\
             local gamma = {{ key = alpha, other = beta }}\n\
             for index = 1, 2 do gamma.key = gamma.key + index end\n\
             return gamma.key + gamma.other\n\
             end\n\
             out[#out + 1] = f{i}({i}, 1)\n"
        ));
    }
    chunk.push_str("return table.concat(out, ',')\n");
    format!(
        "collectgarbage('setpause', 0)\n\
         local f = assert((loadstring or load)([==[{chunk}]==]))\n\
         collectgarbage('setpause', 200)\n\
         return f()"
    )
}

fn expected(n: usize) -> String {
    (0..n)
        .map(|i| (i + 4).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn names_made_again_after_a_step_are_the_ones_kept() {
    let n = 40;
    let src = source(n);
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(run(v, 1, usize::MAX, &src), expected(n), "{v:?}");
        // the system allocator, for a sanitizer to see a freed block read
        let mut vm = luna_core::vm::Vm::new(v);
        let got = match vm.eval(&src).expect("the chunk runs").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        };
        assert_eq!(got, expected(n), "{v:?}");
    }
}

/// A 5.2 chunk whose functions share a label name, which no prototype
/// keeps: with no step due, gc-stress still collects at every check, and
/// frees the name when the function whose table held it ends.
fn labels(n: usize) -> String {
    let mut chunk = String::from("local out = {}\n");
    for i in 0..n {
        chunk.push_str(&format!(
            "local function f{i}() local k = {i} goto skip{m} k = -1 ::skip{m}:: return k end\n\
             out[#out + 1] = f{i}()\n",
            m = i % 3
        ));
    }
    chunk.push_str("return table.concat(out, ',')\n");
    format!("local f = assert(load([==[{chunk}]==]))\nreturn f()")
}

#[test]
fn a_label_freed_with_its_table_is_made_again() {
    let n = 30;
    let want = (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    assert_eq!(run(LuaVersion::Lua52, 1, usize::MAX, &labels(n)), want);
}
