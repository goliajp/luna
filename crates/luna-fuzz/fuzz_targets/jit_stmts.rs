//! Statements and functions of `fuzz_jit_diff`; the loops are in
//! `jit_loops`, the fixed frame in `jit_frame`.

use super::jit_frame::{DUMP, FRAME};
use super::jit_program::Gen;

/// Upper bound on the product of nested trip counts.
pub(crate) const MAX_ITERS: u32 = 3000;

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
        self.anys.clear();
        self.reentry_kernel();
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
    pub(crate) fn body(&mut self, max: u32) {
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
        match self.pick(22) {
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
            18 if self.user_calls && self.ind == 0 => self.reentry_calls(),
            19 if self.user_calls && self.ind == 0 => self.gc_kernel(),
            20 if self.user_calls && self.ind == 0 => self.carry_kernel(),
            21 if self.user_calls && self.ind == 0 => self.nested_kernel(),
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
}
