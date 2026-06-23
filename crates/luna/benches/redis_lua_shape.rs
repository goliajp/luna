//! D1 — Redis-Lua-shape micro-bench. Workloads modelled on the dogfood
//! report §5#1: BullMQ token-bucket / Redlock-style sliding window /
//! method dispatch via metatables / string ops. These are luna's
//! "real-world embedder shape" baselines (vs the fib_28 / loop_int_1m
//! academic shapes in lua_microbench).
//!
//! Zero deps. Median of N runs via `std::time::Instant` — same harness
//! style as lua_microbench. D2 (criterion + CPU pin + n=1000) refines
//! the methodology; D1 ships the workload corpus.
//!
//! Run: `cargo bench --bench redis_lua_shape`

use std::time::{Duration, Instant};

use luna::version::LuaVersion;

struct Bench {
    name: &'static str,
    source: &'static str,
    iters: usize,
}

const BENCHES: &[Bench] = &[
    // ── Token bucket (BullMQ / Redlock pattern) ────────────────────
    //
    // A typical Redis-Lua rate limiter: read counter, decrement,
    // check threshold, write back. Modelled here as a tight loop
    // over a hand-rolled bucket table simulating the hgetall +
    // hincrby + hset round-trip in pure Lua.
    Bench {
        name: "token_bucket_1k",
        source: r#"
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
        "#,
        iters: 50,
    },
    // ── Sliding window limiter ─────────────────────────────────────
    //
    // Sorted-set semantics in pure Lua — `t[#t+1] = x` append plus
    // a windowed remove scan. Hits common Redis-Lua patterns where
    // the script walks an array of (timestamp, count) pairs.
    Bench {
        name: "sliding_window_500",
        source: r#"
            local window = {}
            local now = 500
            local horizon = 100
            local kept = 0
            -- populate
            for i = 1, now do window[i] = i end
            -- slide forward N steps
            for step = 1, 100 do
                now = now + 1
                window[#window + 1] = now
                -- evict anything older than horizon
                local i = 1
                while i <= #window and window[i] < now - horizon do
                    i = i + 1
                end
                if i > 1 then
                    -- shift left (Lua's table.remove(t, 1) but unrolled)
                    local new = {}
                    for j = i, #window do new[#new + 1] = window[j] end
                    window = new
                end
                kept = #window
            end
            return kept
        "#,
        iters: 30,
    },
    // ── Method dispatch via metatables ─────────────────────────────
    //
    // Object-style table.method() calls via __index. Embedders
    // exposing host APIs through metatables hit this shape for
    // every redis.call("HGET", k) -style invocation.
    Bench {
        name: "method_dispatch_5k",
        source: r#"
            local cls = {}
            cls.__index = cls
            function cls:get(k) return self.t[k] end
            function cls:set(k, v) self.t[k] = v end
            function cls:incr(k, by)
                self.t[k] = (self.t[k] or 0) + by
                return self.t[k]
            end
            local function new()
                return setmetatable({t = {}}, cls)
            end
            local o = new()
            local last = 0
            for i = 1, 5000 do
                o:set("k", i)
                local v = o:get("k")
                last = o:incr("k", 1) + v
            end
            return last
        "#,
        iters: 20,
    },
    // ── String ops (KEYS / ARGV concat + parse) ───────────────────
    //
    // Real Redis-Lua scripts spend cycles in string.format /
    // table.concat / string.sub parsing the host-supplied KEYS and
    // ARGV. Modelled here as a build-then-parse loop.
    Bench {
        name: "string_ops_2k",
        source: r#"
            local keys = {"user:1", "user:2", "user:3", "user:4", "user:5"}
            local total = 0
            for i = 1, 2000 do
                local idx = (i % 5) + 1
                local k = keys[idx]
                local s = string.format("session:%s:counter:%d", k, i)
                -- find the last ':' and grab the suffix
                local colon = 0
                for p = 1, #s do
                    if string.sub(s, p, p) == ":" then colon = p end
                end
                local tail = string.sub(s, colon + 1)
                total = total + tonumber(tail)
            end
            return total
        "#,
        iters: 20,
    },
];

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    // Skip `--bench` and similar harness-passthrough args; only use
    // a final user-supplied positional as the filter.
    let filter = argv
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--") && a.as_str() != "redis_lua_shape")
        .cloned();

    let label = "luna       ";
    println!("# Redis-Lua-shape micro-bench (D1)");
    println!("# Median of N runs, wall-clock around Vm::eval. JIT on (luna default).");
    println!();
    println!("{:>26} | {:>10} | {:>6} | {:>8}", "name", "median_ms", "iters", "runtime");
    println!("{:>26} | {:>10} | {:>6} | {:>8}", "-".repeat(26), "-".repeat(10), "-".repeat(6), "-".repeat(8));

    for b in BENCHES {
        if let Some(ref f) = filter {
            if !b.name.contains(f.as_str()) {
                continue;
            }
        }
        let med = run_luna(b);
        println!(
            "{:>26} | {:>10.3} | {:>6} | {:>8}",
            b.name,
            med.as_secs_f64() * 1000.0,
            b.iters,
            label.trim()
        );
    }
}

fn run_luna(b: &Bench) -> Duration {
    let mut samples: Vec<Duration> = (0..b.iters)
        .map(|_| {
            // Fresh Vm per iter so allocations / interp state stay
            // comparable across runs and the per-iter wall-clock
            // doesn't accumulate caches that fade between runs.
            let mut vm = luna::new_minimal_with_jit(LuaVersion::Lua54);
            vm.open_base();
            vm.open_math();
            vm.open_string();
            vm.open_table();
            let start = Instant::now();
            vm.eval(b.source).expect("bench script must run cleanly");
            start.elapsed()
        })
        .collect();
    samples.sort();
    samples[samples.len() / 2]
}
