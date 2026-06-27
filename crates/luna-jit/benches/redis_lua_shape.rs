//! D1+D2 — Redis-Lua-shape micro-bench. Workloads modelled on the dogfood
//! report §5#1: BullMQ token-bucket / Redlock-style sliding window /
//! method dispatch via metatables / string ops. These are luna's
//! "real-world embedder shape" baselines (vs the fib_28 / loop_int_1m
//! academic shapes in lua_microbench).
//!
//! D2 (v1.2): criterion harness with statistical sampling. macOS local
//! variance gate ~2.5% (M-series, no public CPU pin API). Linux CI gate
//! ~1-2% via `taskset -c 1` (set up in `.github/workflows/ci.yml`'s
//! `perf-gate` job). Replaces the D1 hand-rolled median-of-N harness;
//! the workload corpus is unchanged so D1 baselines remain comparable.
//!
//! Run: `cargo bench --bench redis_lua_shape`
//!     `cargo bench --bench redis_lua_shape -- token_bucket_1k`  (filter)
//!     `cargo bench --bench redis_lua_shape -- --quick`          (sanity)

use std::time::Duration;

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use luna_jit::version::LuaVersion;

struct Bench {
    name: &'static str,
    source: &'static str,
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
    },
    // ── Dict 5k lookup (string-keyed registry) ────────────────────
    //
    // C3-redux primary workload — per `.dev/rfcs/v2.1-c3-redux-workload-rfc.md`.
    // 5000 string-keyed entries (AoS working set 327 KB = 2.55× P-core L1d),
    // then 32000 lookups via an LCG-stride sequence (coprime to 5000 so the
    // sequence covers all keys evenly). Cache-spill signal 2.41× per-lookup
    // cost vs token_bucket_1k, where the C3 SoA bandwidth attack has signal
    // to capture. token_bucket_1k stays as the L1-resident sanity check.
    Bench {
        name: "dict_5k_lookup",
        source: r#"
            local t = {}
            for i = 1, 5000 do
                t[string.format("k%04d", i)] = i
            end
            local keys = {}
            local idx = 1
            for i = 1, 32000 do
                idx = ((idx * 4099) % 5000) + 1
                keys[i] = string.format("k%04d", idx)
            end
            collectgarbage("collect")
            local sum = 0
            for i = 1, 32000 do
                local v = t[keys[i]]
                if v then sum = sum + v end
            end
            return sum
        "#,
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
    },
];

fn bench_redis_shape(c: &mut Criterion) {
    let mut group = c.benchmark_group("redis_lua_shape");

    // P-A1 audit: macOS variance gate ~2.5% (no public CPU pin API on
    // M-series); the criterion noise_threshold below is the regression
    // boundary, paired with `measurement_time` long enough that the
    // outlier-rejection statistics converge. Linux CI runs via taskset
    // tighten this further (see the `perf-gate` job in ci.yml).
    group.measurement_time(Duration::from_secs(8));
    group.warm_up_time(Duration::from_secs(2));
    group.sample_size(100);
    group.noise_threshold(0.025);

    for b in BENCHES {
        group.bench_function(b.name, |bencher| {
            bencher.iter_batched(
                || {
                    // Fresh Vm per timed iteration. Setup time is NOT
                    // included in the per-iter measurement (criterion's
                    // iter_batched contract); this matches the D1
                    // hand-rolled harness which timed `vm.eval` only.
                    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
                    vm.open_base();
                    vm.open_math();
                    vm.open_string();
                    vm.open_table();
                    vm
                },
                |mut vm| {
                    black_box(vm.eval(b.source).expect("bench script must run cleanly"));
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_redis_shape);
criterion_main!(benches);
