//! luna-core minimal embed proof — 10-line zero-dep flow.
//!
//! Verifies that:
//! - The `luna-core` crate alone (no Cranelift, no JIT) is enough to
//!   create a Vm, eval Lua source, and read back the return value.
//! - `cargo tree -p luna-core` shows exactly 1 crate (no transitive
//!   third-party dependencies).
//!
//! Run: `cargo run --example embed_min -p luna-core`.
//! Acceptance: prints `Int(3)`.

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn main() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.open_base();
    let r = vm.eval("return 1 + 2").expect("eval failed");
    println!("{:?}", r.first().expect("no return value"));
}
