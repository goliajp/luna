// v2.9 Phase A.2 — check trace JIT engagement on 5 charter cells
// Run as: cd /Users/doracawl/workspace/goliajp/luna && cargo run --release --example diag_v29_jit_engage
use luna_jit::version::LuaVersion;

const CELLS: &[(&str, &str)] = &[
    ("token_bucket_1k", r#"
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
    "#),
    ("method_dispatch_5k", r#"
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
    "#),
];

fn main() {
    for (name, source) in CELLS {
        let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        vm.open_string();
        vm.open_table();
        let _ = vm.eval(source).expect("must run");
        eprintln!(
            "{:28} compiled={:>4} dispatched={:>6}",
            name,
            vm.trace_compiled_count(),
            vm.trace_dispatched_count(),
        );
    }
}
