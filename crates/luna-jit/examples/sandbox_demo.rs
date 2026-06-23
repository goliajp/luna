//! Script-host sandbox demo. Shows how an embedder runs untrusted
//! Lua scripts safely with luna:
//!
//!   1. `Vm::new_minimal(Lua51)` — empty VM, no libraries pre-loaded.
//!   2. Library whitelist — open only the safe subset.
//!   3. Host-registered Rust function callable from Lua.
//!   4. Per-call instruction budget — infinite loops bail with a
//!      catchable `LuaError`.
//!   5. Memory cap — runaway allocation bails the same way.
//!
//! Run: `cargo run --release --example sandbox_demo`.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::vm::error::LuaError;

fn make_sandbox() -> Vm {
    // 5.1 dialect (script-host contract).
    let mut vm = Vm::new_minimal(LuaVersion::Lua51);
    // Whitelist: base (assert/pcall/type/tostring/pairs/...), math, string,
    // table. NO os/io (no file system, no shell). NO debug. NO package.
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    vm.open_coroutine();
    // Sandbox-required gates:
    //   - JIT off so counted-for loops tick instr_budget (`for i=1,1e18 do end`
    //     would otherwise compile to native code and bypass the budget).
    //   - Bytecode loading off so `load("\27Lua…")` cannot bypass the parser.
    vm.set_jit_enabled(false);
    vm.set_bytecode_loading(false);
    vm
}

/// Host-registered Rust function the script can call.
fn host_redis_get(vm: &mut Vm, func_slot: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs < 1 {
        return Ok(vm.nat_return(func_slot, &[Value::Nil]));
    }
    let key = vm.nat_arg(func_slot, nargs, 0);
    // Stub: pretend "user:alice" → "42", everything else nil.
    let result = match key {
        Value::Str(s) if s.as_bytes() == b"user:alice" => {
            let v = vm.heap.intern(b"42");
            Value::Str(v)
        }
        _ => Value::Nil,
    };
    Ok(vm.nat_return(func_slot, &[result]))
}

fn run_script(label: &str, vm: &mut Vm, src: &[u8]) {
    println!("\n=== {label} ===");
    println!("source: {}", String::from_utf8_lossy(src));
    let cl = match vm.load(src, b"=script") {
        Ok(cl) => cl,
        Err(e) => {
            println!("  COMPILE-ERR: {e}");
            return;
        }
    };
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(vs) => println!("  → ok, returned {} value(s): {vs:?}", vs.len()),
        Err(e) => println!("  → error: {}", vm.error_text(&e)),
    }
}

fn main() {
    // ─── Case 1: ordinary script with host function ────────────────
    let mut vm = make_sandbox();
    let host_fn = vm.native(host_redis_get);
    vm.set_global("redis_get", host_fn).unwrap();
    run_script(
        "host call works",
        &mut vm,
        b"return redis_get('user:alice')",
    );

    // ─── Case 2: dangerous library NOT exposed ─────────────────────
    let mut vm = make_sandbox();
    run_script(
        "os.execute is absent (no library escape)",
        &mut vm,
        b"return type(os)",
    );

    // ─── Case 3: instruction budget catches infinite loop ──────────
    let mut vm = make_sandbox();
    vm.set_instr_budget(Some(10_000));
    run_script(
        "instruction budget halts infinite loop",
        &mut vm,
        b"while true do end",
    );

    // ─── Case 4: memory cap catches alloc bomb ─────────────────────
    let mut vm = make_sandbox();
    vm.set_memory_cap(Some(64 * 1024)); // 64 KiB
    run_script(
        "memory cap halts alloc bomb",
        &mut vm,
        b"local t = {}; for i = 1, 1000000 do t[i] = string.rep('x', 100) end",
    );

    // ─── Case 5: arithmetic still works (sanity) ───────────────────
    let mut vm = make_sandbox();
    run_script(
        "math + string library available",
        &mut vm,
        b"return math.sqrt(2) * 2, string.upper('luna')",
    );

    // ─── Case 6: pcall catches Lua-level errors ────────────────────
    let mut vm = make_sandbox();
    run_script(
        "pcall traps script-side error",
        &mut vm,
        b"local ok, err = pcall(function() error('boom') end); return ok, err",
    );

    println!("\n=== sandbox demo complete ===");
}
