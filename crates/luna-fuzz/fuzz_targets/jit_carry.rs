//! A table built in a loop iteration and kept past it. The trace JIT can
//! leave a new table unallocated ("sunk": its fields in registers of the
//! compiled code) while nothing outside the trace sees it; it has to be a
//! real table once a variable of the enclosing scope holds it (`last = t`,
//! `prev = {n = i}`, an upvalue), a second register holds it at an exit,
//! or a plain operation (`t and t.n`, `m[t] = i`) reads it. The kernel
//! builds one table per iteration, keeps it in such places, and prints
//! what is left after the loop. Nothing here recurses, so the generator's
//! allocation stacks stay a fixed set.

use super::jit_program::Gen;

const CTORS: [&str; 5] = [
    "{n = i}",
    "{i, i + 1}",
    "{n = i, i}",
    "{i, n = i, s = 'x' .. i}",
    "{}",
];

impl Gen<'_, '_> {
    /// `do local last ... local function ck(n) <loop> end print(ck(..)) end`
    pub(crate) fn carry_kernel(&mut self) {
        let n = [40, 100, 300][self.pick(3) as usize];
        let k = self.pick(n) + 1;
        let ctor = self.one(&CTORS);
        // `last` an upvalue of `ck` (set with SETUPVAL) or its own local
        let up = self.pick(2) == 0;
        self.line("do");
        self.ind += 1;
        self.line(
            "local function sh(v) if type(v) ~= \"table\" then return tostring(v) end \
             return tostring(v.n) .. \"/\" .. tostring(v[1]) .. \"/\" .. tostring(v.prev and v.prev.n) end",
        );
        if up {
            self.line("local last, prev, r = {n = 0}, {0}, nil");
        }
        self.line("local function ck(n)");
        self.ind += 1;
        if !up {
            self.line("local last, prev, r = {n = 0}, {0}, nil");
        }
        self.line("local m, cnt2, src = {}, 0, {}");
        // the loop: numeric for, while or ipairs (each traced in every
        // version)
        let close = match self.pick(3) {
            0 => {
                self.line("for i = 1, n do");
                "end"
            }
            1 => {
                self.line("local i = 0");
                self.line("while i < n do i = i + 1");
                "end"
            }
            _ => {
                self.line("for q = 1, n do src[q] = q end");
                self.line("for _, i in ipairs(src) do");
                "end"
            }
        };
        self.ind += 1;
        self.line(&format!("local t = {ctor}"));
        if ctor == "{}" {
            self.line("t.n = i");
        }
        if self.pick(2) == 0 {
            self.line("t.prev = last");
        }
        let alias = self.pick(2) == 0;
        if alias {
            self.line("local u = t");
        }
        let held = if alias { "u" } else { "t" };
        for _ in 0..self.pick(3) {
            let s = match self.pick(6) {
                0 => format!("if i == {k} then r = {held} end"),
                1 => format!("if i == {k} then break end"),
                2 => format!("cnt2 = cnt2 + ({held} and {held}.n or 0)"),
                3 => format!("m[{held}] = i"),
                4 => format!("if i % {} == 0 then prev = {held} end", self.pick(9) + 2),
                _ => format!("out_n = {held}.n"),
            };
            self.line(&s);
        }
        let carry = match self.pick(4) {
            0 => format!("last = {held}"),
            1 => format!("last, prev = {held}, last"),
            2 => format!("prev = {ctor}"),
            _ => format!("last = {held} prev = last"),
        };
        self.line(&carry);
        self.ind -= 1;
        self.line(close);
        self.line("local mc = 0");
        self.line("for kk, v in pairs(m) do if kk.n == v or kk[1] == v then mc = mc + 1 end end");
        self.line("return sh(last) .. \" \" .. sh(prev) .. \" \" .. sh(r) .. \" \" .. cnt2 .. \" \" .. mc");
        self.ind -= 1;
        self.line("end");
        let small = self.pick(10) + 1;
        self.line(&format!("print(\"ck\", ck({n}), ck({small}), ck({n}))"));
        self.ind -= 1;
        self.line("end");
    }
}
