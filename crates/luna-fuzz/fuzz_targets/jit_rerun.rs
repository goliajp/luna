//! A loop whose passes write what running one again would show: an
//! append (`out[#out + 1]`), a count kept in an upvalue or a table, a
//! string grown in an upvalue, next to the tables and strings it makes. A
//! trace of it that cannot allocate has to leave without doing any pass
//! twice, and what its exit could not make has to be made, not dropped.

use super::jit_program::Gen;

impl Gen<'_, '_> {
    /// `do local up, acc, out ... local function rk(n) <loop> end ... end`
    pub(crate) fn rerun_kernel(&mut self) {
        let n = [60, 300, 1000][self.pick(3) as usize];
        let k = self.pick(7) + 2;
        self.line("do");
        self.ind += 1;
        self.line("local up, acc, out, tc = 0, \"\", {}, {0}");
        self.line("local function rk(n)");
        self.ind += 1;
        // a local accumulator a trace can buffer, beside the upvalue one
        self.line("local s = \"\"");
        self.line("for i = 1, n do");
        self.ind += 1;
        let mut writes = vec![
            "out[#out + 1] = {i}",
            "tc[1] = tc[1] + i",
            "s = s .. \"y\"",
            "out[#out + 1] = i",
        ];
        // at least one write, in an order the input picks; an input used up
        // picks the first and no other
        let first = self.pick(writes.len() as u32) as usize;
        let w = writes.remove(first);
        self.line(w);
        for w in writes {
            if self.pick(2) == 1 {
                self.line(w);
            }
        }
        // a loop storing to an upvalue is not compiled: seldom
        for w in ["up = up + 1", "acc = acc .. \"x\""] {
            if self.pick(4) == 3 {
                self.line(w);
            }
        }
        if self.pick(2) == 0 {
            // a table the trace can sink, read at an exit that takes the
            // branch
            self.line("local p = {i, i * 2}");
            self.line(&format!("if i % {k} == 0 then tc[1] = tc[1] + p[2] end"));
        } else {
            self.line(&format!("if i % {k} == 0 then tc[1] = tc[1] + 1 end"));
        }
        self.ind -= 1;
        self.line("end");
        self.line("return #s");
        self.ind -= 1;
        self.line("end");
        let small = self.pick(9) + 1;
        self.line(&format!("local r = rk({n}) + rk({small})"));
        self.line("print(\"rk\", r, #out, up, #acc, tc[1])");
        self.ind -= 1;
        self.line("end");
    }
}
