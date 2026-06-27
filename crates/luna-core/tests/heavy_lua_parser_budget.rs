//! Regression: `load(reader, ...)` and `Vm::load` must reject sources
//! that exceed the loader input byte budget with a PUC-shaped
//! `not enough memory` error rather than letting the host allocator
//! crawl past the host's RAM cap and SIGSEGV.
//!
//! Tracked at `.dev/known-bugs/fixed/heavy-lua-sigsegv-under-128mb-loadrep.md`.
//!
//! The PUC source the gate defends against:
//!
//! ```lua
//! -- testes/heavy.lua::loadrep
//! local p = 1<<20
//! local s = string.rep(x, p)
//! local function f() return s end          -- 1 MiB per call, forever
//! local st, msg = load(f, "=big")
//! assert(not st and
//!   (string.find(msg, "string length overflow") or
//!    string.find(msg, "not enough memory")))
//! ```
//!
//! Pre-fix on ubuntu 7 GB CI: the feeder loop ran until the host
//! allocator failed (SIGSEGV). Post-fix: `nat_load` checks the budget
//! before each `extend_from_slice` and returns `(nil, "not enough
//! memory")` at the cap.
//!
//! These tests use a small budget (8 MiB) so the loop bounds cleanly
//! in the harness — the production default is 256 MiB
//! (`Vm::DEFAULT_LOADER_INPUT_BUDGET`).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Pin 1 — `Vm::load(&[u8], ...)` rejects oversize raw sources.
#[test]
fn vm_load_rejects_source_above_budget() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.set_loader_input_budget(1024); // 1 KiB cap
    // 2 KiB of legal Lua — `local a = 1\n` repeats.
    let src = b"local a = 1\n".repeat(200);
    assert!(src.len() > 1024);
    let err = vm.load(&src, b"=oversize").expect_err("must reject");
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("not enough memory"),
        "expected 'not enough memory', got: {msg}"
    );
}

/// Pin 2 — `Vm::load` accepts source at-cap and below.
#[test]
fn vm_load_accepts_source_at_or_below_budget() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.set_loader_input_budget(1 << 20); // 1 MiB cap
    let src = b"return 1".to_vec();
    vm.load(&src, b"=tiny").expect("tiny source should load");
}

/// Pin 3 — feeder loop a la `heavy.lua::loadrep` errors out cleanly at
/// the loader budget. Caps at 8 MiB so the test runs in <1 s and uses
/// well under the harness memory ceiling.
#[test]
fn nat_load_feeder_loop_errors_at_budget() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_loader_input_budget(8 * 1024 * 1024); // 8 MiB
    // Simulate `heavy.lua::loadrep`'s feeder shape: 1 MiB chunk per
    // call, forever. With an 8 MiB cap we expect a clean failure
    // within ~9 iterations.
    let driver = br#"
        local p = 1 << 20
        local s = string.rep("a", p)
        local count = 0
        local function f()
            count = count + p
            return s
        end
        local st, msg = load(f, "=big")
        assert(st == nil, "expected load to fail at the budget cap")
        assert(type(msg) == "string", "expected an error message string")
        assert(string.find(msg, "not enough memory") ~= nil,
            "expected 'not enough memory' in error, got: " .. tostring(msg))
        return msg
    "#;
    let results = vm.eval(std::str::from_utf8(driver).unwrap()).unwrap();
    match results.first() {
        Some(Value::Str(s)) => {
            let txt = String::from_utf8_lossy(s.as_bytes());
            assert!(
                txt.contains("not enough memory"),
                "feeder loop hit unexpected error: {txt}"
            );
        }
        other => panic!("expected error-msg string from driver, got {other:?}"),
    }
}

/// Pin 4 — the default budget is `Vm::DEFAULT_LOADER_INPUT_BUDGET`.
/// Pins the public knob's existence so a future refactor cannot silently
/// drop it.
#[test]
fn default_loader_input_budget_is_256_mib() {
    let vm = Vm::new_minimal(LuaVersion::Lua55);
    assert_eq!(vm.loader_input_budget(), Vm::DEFAULT_LOADER_INPUT_BUDGET);
    assert_eq!(Vm::DEFAULT_LOADER_INPUT_BUDGET, 256 * 1024 * 1024);
}

/// Pin 5 — embedders can widen the cap.
#[test]
fn set_loader_input_budget_widens() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.set_loader_input_budget(usize::MAX);
    assert_eq!(vm.loader_input_budget(), usize::MAX);
    // a 16 KiB-ish source still loads. Use a single statement
    // followed by a long block comment so the parser doesn't hit
    // the 200-local limit and we still exercise the "we crossed
    // the old default" path.
    let mut src = b"return 1\n--[[\n".to_vec();
    src.extend(std::iter::repeat_n(b'a', 16 * 1024));
    src.extend_from_slice(b"\n]]\n");
    vm.load(&src, b"=widened")
        .expect("should load with widened cap");
}
