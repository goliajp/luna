//! Regression: `io.popen` fd ownership round-trip must not double-close.
//!
//! `vm/lib_os_io.rs`'s popen pipe does a `ChildStdout::into_raw_fd()` →
//! `File::from_raw_fd()` round-trip. If `into_raw_fd` let the pipe's `Drop`
//! run after the `File` had taken ownership, both would close the same fd
//! and Rust's `IoSafety` runtime check would abort (`fatal runtime error: IO
//! Safety violation: owned file descriptor already closed, aborting` →
//! SIGABRT). `into_raw_fd` consumes ownership without running Drop, per
//! stdlib contract, so the round-trip is sound; a SIGABRT on PUC's io tests
//! that looked like this was in fact a debug-hook stack overflow.
//!
//! This test pins the popen close path so any future regression of the
//! ownership-transfer shape (e.g. someone accidentally `clone`ing
//! `ChildStdout` or reordering `child.stdout.take()` after the Child is
//! moved into the Userdata slot) would abort *this* test.
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
