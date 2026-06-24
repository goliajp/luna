//! Run token_bucket with trace JIT on/off and compare results.

use luna_jit::version::LuaVersion;

const SRC: &str = r#"
    local bucket = { tokens = 1000, last = 0, rate = 100 }
    local now = 1
    local refilled = 0
    for i = 1, 1000 do
        local elapsed = now - bucket.last
        local refill = elapsed * bucket.rate
        if refill > 0 then
            bucket.tokens = math.min(1000, bucket.tokens + refill)
            bucket.last = now
            refilled = refilled + 1
        end
        if bucket.tokens >= 1 then
            bucket.tokens = bucket.tokens - 1
        end
        now = now + 1
    end
    return bucket.tokens, refilled
"#;

fn run(jit: bool) -> (luna_core::runtime::Value, luna_core::runtime::Value) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_trace_jit_enabled(jit);
    vm.open_base();
    vm.open_math();
    let vals = vm.eval(SRC).expect("eval");
    (vals[0], vals[1])
}

fn main() {
    let (a0, a1) = run(false);
    let (b0, b1) = run(true);
    println!("interp:    tokens={:?}, refilled={:?}", a0, a1);
    println!("trace JIT: tokens={:?}, refilled={:?}", b0, b1);
    let mut ok = true;
    if format!("{:?}", a0) != format!("{:?}", b0) {
        eprintln!("MISMATCH on tokens");
        ok = false;
    }
    if format!("{:?}", a1) != format!("{:?}", b1) {
        eprintln!("MISMATCH on refilled");
        ok = false;
    }
    if ok {
        println!("OK — results match");
    } else {
        std::process::exit(1);
    }
}
