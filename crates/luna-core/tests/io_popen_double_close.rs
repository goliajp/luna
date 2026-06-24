//! Regression: `io.popen` fd ownership round-trip must not double-close.
//!
//! Background — CB1 follow-up (`.dev/known-bugs/fixed/io-safety-fd-double-close.md`).
//! After the v1.3 CB1 compiler short-circuit fix (`fae0f9c`), the integration
//! suite reached PUC's io tests for the first time and tripped Rust's
//! `IoSafety` runtime check (`fatal runtime error: IO Safety violation: owned
//! file descriptor already closed, aborting` → SIGABRT). The suspected site
//! was `vm/lib_os_io.rs`'s popen pipe `ChildStdout::into_raw_fd()` →
//! `File::from_raw_fd()` round-trip — if `into_raw_fd` somehow let the
//! pipe's `Drop` run after the `File` had taken ownership, both would close
//! the same fd.
//!
//! By the time of `f75f1ad` (`v1.3 CB-remaining`) and `e5db587` (`v1.3 CB IO
//! follow-up: line-hook predicate precedence`) the official_run integration
//! test was reported green, and stress-driving popen on `develop` no longer
//! reproduces the abort. The original site (`into_raw_fd` *does* consume
//! ownership without running Drop, per stdlib contract) was sound — the
//! SIGABRT looked like an IO Safety violation but the actual cascade was
//! the debug-hook stack overflow that `e5db587` fixed.
//!
//! This test pins the popen close path so any future regression of the
//! ownership-transfer shape (e.g. someone accidentally `clone`ing
//! `ChildStdout` or reordering `child.stdout.take()` after the Child is
//! moved into the Userdata slot) would abort *this* test rather than
//! lurking until the next dogfood report.
//!
//! Pinned shapes:
//! 1. Many round-trips of `popen("...", "r"):lines() / :close()` —
//!    catches double-close on read pipes (PUC `files.lua` style).
//! 2. Many round-trips of `popen("...", "w"):write() / :close()` —
//!    catches double-close on write pipes.
//! 3. GC reclaim without explicit `:close()` — catches Drop of the
//!    `Userdata::popen_child` `Child` colliding with the pipe `File`'s
//!    Drop on the fd.
//!
//! Any IO Safety violation aborts the test process with SIGABRT (not a
//! panic), which still surfaces as a `cargo test` failure — exactly what
//! we want.

#![cfg(unix)]

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[test]
fn popen_read_close_roundtrip_no_double_close() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let src = r#"
        for i = 1, 200 do
          local f = io.popen('printf hello', 'r')
          local s = f:read('a')
          assert(s == 'hello', 'read mismatch: '..tostring(s))
          local ok, kind, code = f:close()
          assert(ok == true and kind == 'exit' and code == 0,
                 'close triple: '..tostring(ok)..':'..tostring(kind)..':'..tostring(code))
        end
        return 'ok'
    "#;
    let v = vm.eval(src).expect("popen read stress must not error");
    assert_eq!(v.len(), 1);
}

#[test]
fn popen_write_close_roundtrip_no_double_close() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let src = r#"
        for i = 1, 200 do
          local f = io.popen('cat > /dev/null', 'w')
          f:write('payload\n')
          local ok, kind, code = f:close()
          assert(ok == true and kind == 'exit' and code == 0,
                 'close triple: '..tostring(ok)..':'..tostring(kind)..':'..tostring(code))
        end
        return 'ok'
    "#;
    let v = vm.eval(src).expect("popen write stress must not error");
    assert_eq!(v.len(), 1);
}

#[test]
fn popen_gc_reclaim_without_close_no_double_close() {
    // Drop the FILE* userdata on the floor — GC's __gc handler must close it
    // without colliding with the Child stdin/stdout's drop. If popen had a
    // double-take or aliasing shape, this is where IoSafety would abort.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let src = r#"
        for i = 1, 100 do
          local f = io.popen('printf x', 'r')
          local _ = f:read('l')
          -- intentionally no f:close()
        end
        collectgarbage('collect')
        collectgarbage('collect')
        return 'ok'
    "#;
    let v = vm
        .eval(src)
        .expect("popen GC reclaim stress must not error");
    assert_eq!(v.len(), 1);
}
