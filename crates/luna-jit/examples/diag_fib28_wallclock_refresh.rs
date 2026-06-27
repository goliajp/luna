use luna_jit::version::LuaVersion;
use std::time::Instant;

const FIB_SRC: &str = r#"
    local function fib(n) if n<2 then return n end return fib(n-1)+fib(n-2) end
    return fib(28)
"#;

fn measure(label: &str, chunk_jit: bool, trace_jit: bool, p16: bool) {
    let mut samples = Vec::new();
    for _ in 0..15 {
        let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(chunk_jit);
        vm.set_trace_jit_enabled(trace_jit);
        vm.set_p16_self_link_enabled(p16);
        vm.open_base();
        vm.open_math();
        let t = Instant::now();
        let _ = vm.eval(FIB_SRC).expect("eval");
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // drop 2 high + 2 low, keep 11 middle
    let trimmed = &samples[2..13];
    let mean: f64 = trimmed.iter().sum::<f64>() / trimmed.len() as f64;
    let min = trimmed.first().unwrap();
    let max = trimmed.last().unwrap();
    println!(
        "{:50} mean = {:.3} ms  range = {:.3}..{:.3} ms (n=11 trimmed of 15)",
        label, mean, min, max
    );
}

fn main() {
    measure(
        "luna chunk_jit on  / trace off / p16 off (default)",
        true,
        false,
        false,
    );
    measure(
        "luna chunk_jit on  / trace on  / p16 off",
        true,
        true,
        false,
    );
    measure(
        "luna chunk_jit off / trace on  / p16 off (R0 fire row 2)",
        false,
        true,
        false,
    );
    measure(
        "luna chunk_jit off / trace on  / p16 on  (R0 fire row 3)",
        false,
        true,
        true,
    );
    measure(
        "luna chunk_jit off / trace off / p16 off (pure interp)",
        false,
        false,
        false,
    );
}
