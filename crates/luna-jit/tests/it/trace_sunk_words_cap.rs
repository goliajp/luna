//! A trace sinks tables only while their contents fit the room its exits
//! keep for the tables they cannot make (256 words, 20 for each table of
//! eight fields): past that, a table is made as the interpreter makes it.

use luna_jit::version::LuaVersion;

/// A loop making `n` tables of eight fields each, all of them sinkable.
fn tables(n: usize) -> String {
    let mut body = String::new();
    for k in 0..n {
        body.push_str(&format!("local t{k} = {{}} "));
        for f in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            body.push_str(&format!("t{k}.{f} = i "));
        }
        body.push_str(&format!("s = s + t{k}.a + t{k}.h "));
    }
    format!("local s = 0 for i = 1, 300 do {body} end return s")
}

/// The result and the tables the traces sank.
fn run(src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let r = vm.eval(src).unwrap();
    (format!("{:?}", r[0]), vm.trace_sunk_alloc_count())
}

#[test]
fn tables_past_the_room_an_exit_keeps_are_not_sunk() {
    // twelve tables take 240 words; a thirteenth would take 260
    let fit = tables(12);
    assert_eq!(run(&fit, true), (run(&fit, false).0, 12));
    let past = tables(13);
    assert_eq!(run(&past, true), (run(&past, false).0, 12));
}
