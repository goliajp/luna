//! Loops of `fuzz_jit_diff`: each runs in a function of its own, with
//! trip counts bounded by [`MAX_ITERS`] over all nesting.

use super::jit_program::Gen;
use super::jit_stmts::MAX_ITERS;

impl Gen<'_, '_> {
    /// The trace JIT records one loop per function (the first whose
    /// back edge gets hot), so each loop of the main chunk runs in a
    /// function of its own, the locals it assigns passed in and back out.
    pub(crate) fn loop_stmt(&mut self) {
        if self.iters > 1 || !self.user_calls {
            return self.loop_inner();
        }
        const LOCALS: &str = "n1, n2, n3, x, s, o, cnt, psum";
        self.line(&format!("{LOCALS} = (function({LOCALS})"));
        self.ind += 1;
        // tables in locals: a trace reading them through upvalues is not
        // dispatched
        self.line("local t, arr, c, seq, M, M2, P, objs, BT, BK = t, arr, c, seq, M, M2, P, objs, BT, BK");
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

    pub(crate) fn loop_inner(&mut self) {
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
        let kind = if self.hot && self.pick(2) == 0 {
            0
        } else {
            self.pick(6)
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
            // 5.3 wraps past `math.maxinteger` and never ends by itself
            4 if self.ints => format!(
                "for {i} = math.maxinteger - {last}, math.maxinteger do if {i} < 0 then break end"
            ),
            // a float limit, which 5.3 floors for an integer loop
            4 => format!("for {i} = 1, {trips}.75 do"),
            _ => format!("for {i} = 1, math.min({trips}, math.floor(tonumber(n1) or 0)) do"),
        };
        self.line(&head);
        self.nums.push(i);
        self.body(5);
        self.line("end");
    }
}
