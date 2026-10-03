//! Statements, loops and the fixed program frame of `fuzz_jit_diff`.

use super::jit_program::Gen;

/// Upper bound on the product of nested trip counts.
const MAX_ITERS: u32 = 3000;

const FRAME: &str = r#"__out = {}
print = function(...)
  local p = {}
  for i = 1, select('#', ...) do p[i] = tostring((select(i, ...))) end
  __out[#__out + 1] = table.concat(p, "\t")
end
local n1, n2, n3 = NUM1, NUM2, NUM3
local x, s = nil, "s"
local cnt, psum, hits, nhits = 0, 0, 0, 0
local c = {0, 0, n = 0}
local t = {1, 2, 3, x = 1, y = "y"}
local arr = {1, 2.5, "3", 4, 5, 6, 7, 8}
local seq = {}
local Mstore = {}
local MT = {}
MT.__index = function(_, k)
  hits = hits + 1
  local v = Mstore[k]
  if v == nil then return type(k) == "number" and k * 2 or 1 end
  return v
end
MT.__newindex = function(_, k, v) nhits = nhits + 1 Mstore[k] = v end
MT.__len = function() hits = hits + 1 return 7 end
MT.__concat = function(p, q) hits = hits + 1 return type(p) .. type(q) end
MT.__eq = function() hits = hits + 1 return hits % 2 == 0 end
MT.__lt = function() hits = hits + 1 return hits % 3 == 0 end
MT.__le = function() hits = hits + 1 return hits % 3 ~= 0 end
MT.__call = function(_, y) hits = hits + 1 return (tonumber(y) or 0) + 1 end
MT.__add = function() hits = hits + 1 return 3 end
MT.__unm = function() hits = hits + 1 return -1 end
local MTidx = MT.__index
local M, M2 = setmetatable({}, MT), setmetatable({}, MT)
local P = setmetatable({}, {__index = t, __newindex = t})
local TMT = {__index = function(_, k) hits = hits + 1 return 5 end}
local TMT2 = {__index = Mstore, __newindex = Mstore}
local objs = {t, M, P}
local o = t
local obj = {v = 0}
local function va(...)
  local acc = select('#', ...)
  for q = 1, acc do
    local v = select(q, ...)
    if type(v) == "number" then acc = acc + v end
  end
  return acc, ...
end
"#;

// addresses are masked before the sort: with a function or table key the
// order would otherwise follow allocation addresses, which differ between
// the jit and interpreter runs
const DUMP: &str = r#"local function dump(tb)
  local ks = {}
  for k, v in pairs(tb) do
    ks[#ks + 1] = (string.gsub(type(k):sub(1, 1) .. tostring(k) .. "=" .. tostring(v), "0x%x*", "ADDR"))
  end
  table.sort(ks)
  return table.concat(ks, " ")
end
print("end", n1, n2, n3, x, s, cnt, psum, hits, nhits, #seq, obj.v)
print(dump(t)) print(dump(Mstore)) print(dump(arr)) print(dump(c))
"#;

impl Gen<'_, '_> {
    /// A whole program.
    pub(crate) fn program(&mut self) -> String {
        let mut frame = FRAME.to_string();
        for k in ["NUM1", "NUM2", "NUM3"] {
            let v = self.special_num();
            frame = frame.replacen(k, &v, 1);
        }
        self.out.push_str(&frame);
        self.functions();
        self.user_calls = true;
        self.fuel = 40;
        while self.fuel > 0 {
            self.stmt();
        }
        self.out.push_str(DUMP);
        std::mem::take(&mut self.out)
    }

    fn functions(&mut self) {
        self.max_trips = 5;
        self.fuel = 6;
        self.nums = vec!["a".into()];
        self.anys = vec!["b".into()];
        self.line("local function f(a, b)");
        self.body(3);
        let r = self.num(1);
        self.line(&format!("  return {r}"));
        self.line("end");

        self.fuel = 4;
        self.nums = vec!["k".into(), "y".into()];
        self.anys.clear();
        self.line("local function mk(k)");
        self.line("  local acc = 0");
        self.line("  return function(y)");
        self.line("    acc = acc + 1");
        self.ind += 1;
        self.body(2);
        self.ind -= 1;
        let r = self.num(1);
        self.line(&format!("    return acc + {r}"));
        self.line("  end");
        self.line("end");
        let k = self.pick(5);
        self.line(&format!("local g = mk({k})"));

        self.fuel = 4;
        self.nums = vec!["r".into(), "acc".into()];
        self.line("local function rec(r, acc)");
        self.line("  if r <= 0 then return acc end");
        self.body(2);
        let e = self.num(1);
        let tail = match self.pick(3) {
            0 => format!("rec(r - 1, {e})"),
            1 => format!("rec(r - 1, acc) + {e}"),
            _ => format!("{e} - rec(r - 1, acc + 1)"),
        };
        self.line(&format!("  return {tail}"));
        self.line("end");

        self.fuel = 4;
        self.nums = vec!["y".into()];
        self.line("function obj:m(y)");
        self.line("  self.v = self.v + 1");
        self.body(2);
        let r = self.num(1);
        self.line(&format!("  return self.v + {r}"));
        self.line("end");
        self.hot_function();
        self.nums.clear();
        self.max_trips = 300;
    }

    /// `hk(a, b)`: a hot loop in a function that is called many times, so
    /// its trace is entered again with other arguments (a NaN, a string,
    /// a table) than the ones it was recorded with.
    fn hot_function(&mut self) {
        self.fuel = 6;
        self.max_trips = 9;
        self.nums.clear();
        self.anys.clear();
        self.line("local function hk(a, b)");
        self.line("  local t, arr, c, seq, M, M2, P, objs = t, arr, c, seq, M, M2, P, objs");
        self.line("  local n1, n2, n3, f1, f2 = a, b, 0, b, b * 2");
        self.ind += 1;
        let (hot, floats) = (self.hot, self.floats);
        (self.hot, self.floats) = (true, true);
        // called from loops: its own loops stay short
        self.iters = MAX_ITERS / 30;
        self.loop_inner();
        self.iters = 1;
        (self.hot, self.floats) = (hot, floats);
        self.ind -= 1;
        let r = self.one(&["n1", "n2", "n3", "f1", "f2"]);
        self.line(&format!("  return {r}"));
        self.line("end");
    }

    /// Up to `max` statements, one indent level deeper.
    fn body(&mut self, max: u32) {
        self.ind += 1;
        let n = self.pick(max) + 1;
        for _ in 0..n {
            if self.fuel == 0 {
                break;
            }
            self.stmt();
        }
        self.ind -= 1;
    }

    fn stmt(&mut self) {
        self.fuel = self.fuel.saturating_sub(1);
        if self.hot && self.pick(8) != 0 && self.hstmt() {
            return;
        }
        // the main chunk is mostly kernels
        if self.iters == 1 && self.user_calls && self.pick(3) == 0 {
            return self.loop_stmt();
        }
        match self.pick(18) {
            0 => {
                let (v, e) = (self.num_var_target(), self.num(0));
                self.line(&format!("{v} = {e}"));
            }
            1 => {
                let e = self.any(0);
                self.line(&format!("x = {e}"));
            }
            2 => {
                let e = self.str_e(0);
                self.line(&format!("s = string.sub({e}, 1, 40)"));
            }
            3 => {
                let e = self.obj(0);
                self.line(&format!("o = {e}"));
            }
            4 | 5 => {
                let (o, k, v) = (self.obj(1), self.key(1), self.any(1));
                // a statement starting with `(` would continue the previous one
                if o.starts_with('(') {
                    self.line(&format!("do local q = {o} q[{k}] = {v} end"));
                } else {
                    self.line(&format!("{o}[{k}] = {v}"));
                }
            }
            6 => {
                let s = self.one(&["c[1] = c[1] + 1", "c.n = c.n + 1", "cnt = cnt + 1"]);
                self.line(s);
            }
            7 => {
                let e = self.any(1);
                self.line(&format!("seq[#seq + 1] = {e} or false"));
            }
            8 => {
                let (k, e) = (self.pick(8) + 1, self.any(1));
                self.line(&format!("arr[{k}] = {e} or 0"));
            }
            9 => {
                let (a, b) = (self.any(1), self.any(1));
                self.line(&format!("print({a}, {b})"));
            }
            10 => self.if_stmt(),
            11 | 12 => self.loop_stmt(),
            13 if self.in_loop => {
                let c = self.cond(1);
                self.line(&format!("if {c} then break end"));
            }
            14 => self.pcall_block(),
            15 => self.meta_stmt(),
            16 if self.user_calls => self.call_stmt(),
            17 if self.iters <= 64 => self.line("collectgarbage()"),
            _ => {
                let e = self.num(1);
                self.line(&format!("n1, x = va({e}, x)"));
            }
        }
    }

    fn num_var_target(&mut self) -> String {
        self.one(&["n1", "n2", "n3"]).to_string()
    }

    fn if_stmt(&mut self) {
        let c = self.cond(0);
        self.line(&format!("if {c} then"));
        self.body(3);
        if self.pick(2) == 1 {
            self.line("else");
            self.body(2);
        }
        self.line("end");
    }

    fn pcall_block(&mut self) {
        let in_loop = std::mem::replace(&mut self.in_loop, false);
        self.line("print(pcall(function()");
        self.body(3);
        self.line("end))");
        self.in_loop = in_loop;
    }

    fn meta_stmt(&mut self) {
        let c = self.cond(1);
        let s = match self.pick(4) {
            0 => format!("setmetatable(t, {c} and TMT or nil)"),
            1 => format!("setmetatable(t, {c} and TMT2 or nil)"),
            2 => format!("MT.__index = {c} and Mstore or MTidx"),
            _ => format!("arr = {c} and {{9, 8, 7, 6, 5, 4, 3, 2}} or arr"),
        };
        self.line(&s);
    }

    fn call_stmt(&mut self) {
        let s = match self.pick(5) {
            0 => format!("f({}, {})", self.num(1), self.any(1)),
            1 => format!("g = mk({})", self.num(1)),
            2 => format!("n2 = rec({}, {})", self.pick(9), self.num(1)),
            3 => format!("obj:m({})", self.num(1)),
            _ => format!("x = select(2, va({}, {}))", self.any(1), self.any(1)),
        };
        self.line(&s);
    }

    /// The trace JIT records one loop per function (the first whose
    /// back edge gets hot), so each loop of the main chunk runs in a
    /// function of its own, the locals it assigns passed in and back out.
    fn loop_stmt(&mut self) {
        if self.iters > 1 || !self.user_calls {
            return self.loop_inner();
        }
        const LOCALS: &str = "n1, n2, n3, x, s, o, cnt, psum";
        self.line(&format!("{LOCALS} = (function({LOCALS})"));
        self.ind += 1;
        // tables in locals: a trace reading them through upvalues is not
        // dispatched
        self.line("local t, arr, c, seq, M, M2, P, objs = t, arr, c, seq, M, M2, P, objs");
        let kernel_hot = self.pick(4) != 0;
        let hot = std::mem::replace(&mut self.hot, kernel_hot);
        if kernel_hot {
            // floats that stay floats, so a trace compiles float compares
            let (a, b) = (self.pick(8), self.pick(8));
            self.line(&format!("local f1, f2 = {a}.5, -{b}.25"));
            self.floats = true;
        }
        self.loop_inner();
        self.floats = false;
        self.hot = hot;
        self.line(&format!("return {LOCALS}"));
        self.ind -= 1;
        self.line(&format!("end)({LOCALS})"));
    }

    fn loop_inner(&mut self) {
        if self.iters >= MAX_ITERS || self.fuel < 2 {
            return self.line("cnt = cnt + 1");
        }
        const TRIPS: [u32; 9] = [1, 2, 3, 5, 9, 17, 40, 100, 300];
        // a kernel's own loop runs long enough to get hot
        let first = if self.iters == 1 && self.user_calls {
            5
        } else {
            0
        };
        let trips = TRIPS[(first + self.pick(9 - first)) as usize]
            .min(self.max_trips)
            .min(MAX_ITERS / self.iters)
            .max(1);
        let id = self.id();
        let (nums, anys) = (self.nums.len(), self.anys.len());
        let saved = (self.in_loop, self.iters);
        self.in_loop = true;
        self.iters *= trips;
        // before 5.4 the trace JIT records `while` loops, not numeric for
        let kind = match (self.hot, self.wide_for) {
            (true, false) if self.pick(4) != 0 => 2,
            (true, true) if self.pick(2) == 0 => 0,
            _ => self.pick(6),
        };
        match kind {
            0 | 1 => self.numeric_for(id, trips),
            2 => {
                let w = format!("w{id}");
                self.line(&format!(
                    "do local {w} = 0 while {w} < {trips} do {w} = {w} + 1"
                ));
                self.nums.push(w);
                self.body(5);
                self.line("end end");
            }
            3 => {
                let w = format!("w{id}");
                self.line(&format!("do local {w} = 0 repeat {w} = {w} + 1"));
                self.nums.push(w.clone());
                self.body(5);
                self.line(&format!("until {w} >= {trips} end"));
            }
            4 if saved.1 * 8 <= MAX_ITERS => {
                self.iters = saved.1 * 8;
                // the body can store past the end of `arr` (a computed
                // key, or `arr` reached through `o` / `objs`), which
                // would keep `ipairs` going for ever: stop at its
                // initial length
                self.line(&format!(
                    "for k{id}, v{id} in ipairs(arr) do if k{id} > 8 then break end"
                ));
                self.nums.push(format!("k{id}"));
                self.anys.push(format!("v{id}"));
                self.body(5);
                self.line("end");
            }
            // `t` and `Mstore` can hold thousands of keys by now
            _ if saved.1 > 20 => self.numeric_for(id, trips),
            _ => {
                let tb = self.one(&["t", "Mstore", "arr", "c"]);
                self.line(&format!(
                    "for k{id}, v{id} in pairs({tb}) do cnt = cnt + 1 \
                     if type(v{id}) == \"number\" then psum = psum + 1 end end"
                ));
            }
        }
        self.nums.truncate(nums);
        self.anys.truncate(anys);
        (self.in_loop, self.iters) = saved;
    }

    fn numeric_for(&mut self, id: u32, trips: u32) {
        let i = format!("i{id}");
        let last = trips - 1;
        let head = match self.pick(6) {
            0 => format!("for {i} = 1, {trips} do"),
            1 => format!("for {i} = {trips}, 1, -1 do"),
            2 => format!("for {i} = 0, {}, 2 do", last * 2),
            3 => format!("for {i} = 0.5, {}, 0.5 do", trips as f64 * 0.5),
            4 if self.wide_for => format!("for {i} = math.maxinteger - {last}, math.maxinteger do"),
            _ => format!("for {i} = 1, math.min({trips}, math.floor(tonumber(n1) or 0)) do"),
        };
        self.line(&head);
        self.nums.push(i);
        self.body(5);
        self.line("end");
    }
}
