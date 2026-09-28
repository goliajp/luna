//! The trace-friendly part of the `fuzz_jit_diff` grammar. The trace JIT
//! compiles a loop only while its registers keep one type, so a kernel
//! starts out with integer arithmetic on locals, integer-keyed table
//! reads and writes, lengths and comparisons; after some iterations a
//! guarded statement changes a value's type or an object's metatable,
//! and the running trace has to leave in the middle of an iteration.

use super::jit_program::{Gen, MAX_DEPTH};

impl Gen<'_, '_> {
    pub(crate) fn hnum(&mut self, d: u32) -> String {
        let n = if d >= MAX_DEPTH { 4 } else { 16 };
        match self.pick(n) {
            0 => format!("{}", self.pick(12) as i32 - 2),
            1 => self.num_var(),
            2 => self.loop_var().unwrap_or_else(|| "n2".to_string()),
            3 => self.one(&["c[1]", "#seq", "cnt", "#arr"]).to_string(),
            4..=6 => {
                let op = self.one(&["+", "-", "*", "+"]);
                format!("({} {op} {})", self.hnum(d + 1), self.hnum(d + 1))
            }
            7 => format!("(- {})", self.hnum(d + 1)),
            8 if self.ints => {
                let op = self.one(&["//", "%", "&", "|", "~", "<<", ">>"]);
                let l = match self.loop_var() {
                    Some(v) if self.pick(3) != 0 => v,
                    _ => self.hnum(d + 1),
                };
                format!("({l} {op} {})", self.pick(7) + 1)
            }
            8 => format!("({} % {})", self.hnum(d + 1), self.pick(7) + 1),
            9 | 10 => format!("{}[{}]", self.hobj(), self.hkey(d)),
            11 => match self.loop_var() {
                Some(v) => format!("objs[{v} % 3 + 1][{}]", self.hkey(d)),
                None => format!("M[{}]", self.hkey(d)),
            },
            12 => format!("({} / {})", self.hnum(d + 1), self.hnum(d + 1)),
            13 => self.special_num(),
            14 => {
                let a = self.hnum(d + 1);
                match self.pick(3) {
                    0 => format!("math.floor({a})"),
                    1 => format!("math.max({a}, {})", self.hnum(d + 1)),
                    _ => format!("math.min({a}, {})", self.hnum(d + 1)),
                }
            }
            _ if self.user_calls && self.pick(3) == 0 => self.user_call(d),
            _ => "0.5".to_string(),
        }
    }

    fn hobj(&mut self) -> String {
        self.one(&["t", "arr", "arr", "c", "M", "P", "o"]).to_string()
    }

    /// Keys that compile to register or integer indexing, not a field
    /// access by constant name.
    fn hkey(&mut self, d: u32) -> String {
        match self.pick(5) {
            0 => format!("{}", self.pick(4) + 1),
            1 => self.loop_var().unwrap_or_else(|| "1".to_string()),
            2 => match self.loop_var() {
                Some(v) => format!("{v} % 8 + 1"),
                None => "2".to_string(),
            },
            3 if d < MAX_DEPTH => self.hnum(d + 1),
            _ => "3".to_string(),
        }
    }

    pub(crate) fn hcond(&mut self, d: u32) -> String {
        if self.floats && self.pick(3) == 0 {
            // float against float: the compiled compare must keep NaN's
            // answers (`not (x < y)` holds for NaN, `x >= y` does not)
            let op = self.one(&["<", "<=", ">", ">=", "==", "~="]);
            let (l, r) = (self.one(&["f1", "f2"]), self.one(&["f1", "f2", "0.5", "-1.5"]));
            return match self.pick(2) {
                0 => format!("({l} {op} {r})"),
                _ => format!("(not ({l} {op} {r}))"),
            };
        }
        match self.pick(if d >= MAX_DEPTH { 2 } else { 5 }) {
            0 => {
                let op = self.one(&["<", "<=", ">", ">=", "==", "~="]);
                format!("({} {op} {})", self.num_var(), self.hnum(MAX_DEPTH))
            }
            1 => self.after(),
            2 => {
                let op = self.one(&["<", "<=", ">", ">=", "==", "~="]);
                format!("({} {op} {})", self.hnum(d + 1), self.hnum(d + 1))
            }
            3 => {
                let e = self.hnum(d + 1);
                format!("({e} ~= {e})")
            }
            _ => {
                let op = self.one(&["and", "or"]);
                format!("({} {op} {})", self.hcond(d + 1), self.hcond(d + 1))
            }
        }
    }

    /// True from some iteration on (or on some iterations): the trace is
    /// recorded before it holds.
    fn after(&mut self) -> String {
        match self.loop_var() {
            Some(v) => match self.pick(3) {
                0 => format!("({v} > {})", self.pick(60) + 3),
                1 => format!("({v} % {} == 0)", self.pick(9) + 5),
                _ => format!("({v} == {})", self.pick(60) + 3),
            },
            None => format!("(n1 > {})", self.pick(10)),
        }
    }

    /// Something a running trace did not see: a new type in a register or
    /// table slot, or a metatable.
    fn shape_change(&mut self) -> String {
        let k = self.pick(8) + 1;
        match self.pick(10) {
            0 => format!("arr[{k}] = {}", self.one(&["\"10\"", "2.5", "M", "(0/0)", "-0.0"])),
            1 => format!("n{} = {}", self.pick(3) + 1, self.one(&["0.5", "\"7\"", "(1/0)", "(0/0)", "-0.0", "math.maxinteger", "M"])),
            2 => "setmetatable(t, TMT)".to_string(),
            3 => "setmetatable(t, TMT2)".to_string(),
            4 => format!(
                "objs[{}] = {}",
                self.pick(3) + 1,
                self.one(&["M", "P", "t", "arr", "5", "\"s\"", "2.5"])
            ),
            5 => format!("c[1] = {}", self.one(&["0.5", "\"3\"", "M", "math.mininteger"])),
            6 => "o = M".to_string(),
            7 => "MT.__index = Mstore".to_string(),
            8 if self.floats => format!(
                "f{} = {}",
                self.pick(2) + 1,
                self.one(&["(0/0)", "-(0/0)", "(1/0)", "(-1/0)", "-0.0"])
            ),
            _ => format!("t[{k}] = nil"),
        }
    }

    /// A statement of the trace-friendly grammar; `false` when it picked
    /// none and the caller should emit a general one.
    pub(crate) fn hstmt(&mut self) -> bool {
        match self.pick(16) {
            0..=2 => {
                let v = self.one(&["n1", "n2", "n3"]);
                let e = self.hnum(0);
                self.line(&format!("{v} = {e}"));
            }
            3 | 4 => {
                let (o, k, v) = (self.hobj(), self.hkey(1), self.hnum(1));
                // `#arr` must stay well defined: no holes in it
                if o == "arr" {
                    self.line(&format!("arr[{k}] = {v} or 0"));
                } else {
                    self.line(&format!("{o}[{k}] = {v}"));
                }
            }
            5 => {
                let s = self.one(&["c[1] = c[1] + 1", "cnt = cnt + 1", "c[2] = c[2] + 1"]);
                self.line(s);
            }
            6 => {
                let e = self.hnum(1);
                self.line(&format!("seq[#seq + 1] = {e}"));
            }
            7 => match self.loop_var() {
                Some(v) => {
                    let (k, e) = (self.hkey(1), self.hnum(1));
                    self.line(&format!("objs[{v} % 3 + 1][{k}] = {e}"));
                }
                None => return false,
            },
            8 | 9 => {
                let (c, s) = (self.after(), self.shape_change());
                self.line(&format!("if {c} then {s} end"));
            }
            10 => {
                let c = self.hcond(0);
                self.line(&format!("if {c} then"));
                self.ind += 1;
                if !self.hstmt() {
                    self.line("cnt = cnt + 1");
                }
                self.ind -= 1;
                self.line("end");
            }
            12 if self.floats => {
                let (v, w) = (self.pick(2) + 1, self.pick(2) + 1);
                let op = self.one(&["+", "-", "*"]);
                self.line(&format!("f{v} = f{v} {op} f{w} * 0.5"));
            }
            13 if self.user_calls => {
                // mostly the same kind of argument, now and then another
                let v = self.one(&["n1", "n2", "n3"]);
                let a = self.hnum(1);
                let (c, odd) = (self.after(), self.one(&["(0/0)", "-0.0", "(1/0)", "\"5\"", "M", "2^63"]));
                self.line(&format!("{v} = hk({a}, {c} and {odd} or 0.5)"));
            }
            11 if self.in_loop => {
                let c = self.after();
                self.line(&format!("if {c} then break end"));
            }
            _ => return false,
        }
        true
    }
}
