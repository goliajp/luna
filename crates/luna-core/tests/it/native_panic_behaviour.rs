//! A Rust panic inside a host native becomes the Lua error
//! `native panic: <msg>`, raised where the native ran (see
//! `tests/native_panic/case.rs`).

#[path = "../native_panic/case.rs"]
mod case;

use luna_core::vm::Vm;

#[test]
fn native_panics_read_the_same_in_every_dialect() {
    let mut failures = Vec::new();
    for (dialect, v) in case::DIALECTS {
        let got = case::run(&mut Vm::new(v));
        let want = case::expected(dialect);
        if got != want {
            failures.push(format!("== {dialect}\n{got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "differs from panics.txt:\n{}",
        failures.join("\n")
    );
}
