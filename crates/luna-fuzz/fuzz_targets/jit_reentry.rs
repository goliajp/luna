//! Re-entering a compiled trace with other arguments. `kr(p, q)` runs one
//! loop that orders two floats every way (`<`, `<=`, `>`, `>=`, `==`,
//! `~=`, each also under `not`); the main chunk calls it until its trace is
//! compiled for floats, then calls it again with NaN, infinities, signed
//! zeros, integers, a numeric string, a table with metamethods or nil. NaN
//! is still a float, so it runs the compiled compares, which must answer
//! as the interpreter does (`not (x < y)` holds for NaN, `x >= y` does
//! not). Nothing here recurses, so the generator's allocation stacks stay
//! a fixed set.

use super::jit_program::Gen;

const OPS: [&str; 6] = ["<", "<=", ">", ">=", "==", "~="];

/// Plain floats the trace is recorded with.
const WARM: [&str; 6] = ["1.5", "3.25", "-0.5", "2.0", "-7.75", "0.125"];

impl Gen<'_, '_> {
    /// One ordered (or equality) compare between the kernel's floats.
    fn reentry_cmp(&mut self) -> String {
        let op = self.one(&OPS);
        let l = self.one(&["f1", "f2", "p", "q"]);
        let r = self.one(&["f1", "f2", "p", "q", "0.5", "-1.5", "(i * 0.5)"]);
        match self.pick(2) {
            0 => format!("({l} {op} {r})"),
            _ => format!("(not ({l} {op} {r}))"),
        }
    }

    /// `local function kr(p, q)`, defined once before the main chunk.
    pub(crate) fn reentry_kernel(&mut self) {
        let trips = [6, 12, 25][self.pick(3) as usize];
        self.line("local function kr(p, q)");
        self.line("  local acc, f1, f2 = 0, p, q");
        match self.pick(3) {
            0 => {
                // the loop condition is itself a compare that NaN answers
                let c = self.reentry_cmp();
                self.line(&format!(
                    "  local i = 0 while i < {trips} and ({c} or i % 3 ~= 1) do i = i + 1"
                ));
            }
            _ => self.line(&format!("  for i = 1, {trips} do")),
        }
        let n = self.pick(4) + 2;
        for k in 0..n {
            let c = self.reentry_cmp();
            let line = match self.pick(3) {
                0 => format!("    acc = acc + ({c} and {} or 0)", 1 << k),
                _ => format!("    if {c} then acc = acc + {} end", 1 << k),
            };
            self.line(&line);
        }
        let step = self.one(&[
            "f1 = f1 + 0.5",
            "f2 = f2 - 0.25",
            "f1 = f1 * -1",
            "f1, f2 = f2, f1",
            "f1 = f1 + i * 0.5",
        ]);
        self.line(&format!("    {step}"));
        self.line("  end");
        self.line("  return acc");
        self.line("end");
    }

    /// Warm `kr` up with floats, then call it with what the trace was not
    /// recorded with.
    pub(crate) fn reentry_calls(&mut self) {
        let (a, b) = (self.one(&WARM), self.one(&WARM));
        let warm = [2, 5, 12][self.pick(3) as usize];
        self.line(&format!(
            "for r = 1, {warm} do psum = psum + kr({a}, {b}) end"
        ));
        let n = self.pick(3) + 1;
        for _ in 0..n {
            let odd = self.reentry_arg();
            let other = self.one(&WARM);
            let (x, y) = match self.pick(3) {
                0 => (odd.as_str(), other),
                1 => (other, odd.as_str()),
                _ => (odd.as_str(), odd.as_str()),
            };
            self.line(&format!("print(\"kr\", pcall(kr, {x}, {y}))"));
        }
    }

    fn reentry_arg(&mut self) -> String {
        let floats = [
            "(0/0)", "-(0/0)", "(0/0)", "(1/0)", "(-1/0)", "-0.0", "0.0", "1e308",
        ];
        match self.pick(4) {
            0 if self.ints => self
                .one(&["3", "-1", "math.maxinteger", "math.mininteger"])
                .to_string(),
            1 => self.one(&["\"4\"", "M", "nil", "\"nan\""]).to_string(),
            _ => self.one(&floats).to_string(),
        }
    }
}
