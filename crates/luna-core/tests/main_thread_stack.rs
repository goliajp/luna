// own binary without the test harness: it runs on the process's main
// thread, whose stack the OS grows on demand

//! The main thread's stack bounds are its stack size limit, not the part
//! the kernel has mapped so far: a fresh process compiles and runs code as
//! deeply nested as the parser allows. (musl's `pthread_getattr_np` reports
//! only the mapped part of the main thread's stack.)

use luna_core::native_stack;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn main() {
    let room = native_stack::sp().wrapping_sub(native_stack::low());
    eprintln!("main thread: {room} bytes of stack below the stack pointer");
    for v in [LuaVersion::Lua51, LuaVersion::Lua53, LuaVersion::Lua55] {
        // 150 levels: within every dialect's parser limit
        let src = format!("return {}1{}", "(".repeat(150), ")".repeat(150));
        let r = Vm::new(v).eval(&src);
        assert!(r.is_ok(), "{v:?}: the nested expression did not compile");
    }
    eprintln!("main thread: nested expressions compiled");
}
