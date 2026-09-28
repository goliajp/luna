//! Program generator for `fuzz_jit_diff`: bounded loops, recursion,
//! closures and metatables, shaped so that loops get hot, traces compile,
//! and operand types or metatables change while the trace is running.
//!
//! Every program terminates: loops run a fixed trip count (their counters
//! are never assignment targets), recursion is bounded by a literal depth,
//! and the product of nested trip counts is capped. Output never depends
//! on hash order (`pairs` bodies only count, the final dump sorts).

use arbitrary::Unstructured;
use luna_core::version::LuaVersion;

pub(crate) struct Gen<'a, 'b> {
    pub(crate) u: &'b mut Unstructured<'a>,
    /// 5.3+: integer subtype, `//`, bitwise operators, `math.tointeger`
    pub(crate) ints: bool,
    /// 5.4+: a numeric for ending at `math.maxinteger` terminates
    pub(crate) wide_for: bool,
    pub(crate) out: String,
    pub(crate) ind: usize,
    /// numeric locals readable here (loop counters, parameters)
    pub(crate) nums: Vec<String>,
    /// locals of any type readable here (generic-for values)
    pub(crate) anys: Vec<String>,
    /// a `break` here leaves a loop of this function
    pub(crate) in_loop: bool,
    /// calls of the generated functions are allowed (false inside them)
    pub(crate) user_calls: bool,
    pub(crate) fuel: u32,
    /// inside a loop kernel: prefer what the trace JIT compiles
    pub(crate) hot: bool,
    /// the kernel's float locals `f1`, `f2` are in scope
    pub(crate) floats: bool,
    /// product of the trip counts of the enclosing loops
    pub(crate) iters: u32,
    pub(crate) max_trips: u32,
    pub(crate) next_id: u32,
}

pub(crate) const MAX_DEPTH: u32 = 3;

impl<'a, 'b> Gen<'a, 'b> {
    pub(crate) fn new(u: &'b mut Unstructured<'a>, v: LuaVersion) -> Self {
        let ints = !matches!(v, LuaVersion::Lua51 | LuaVersion::Lua52);
        let wide_for = matches!(v, LuaVersion::Lua54 | LuaVersion::Lua55);
        Gen {
            u,
            ints,
            wide_for,
            out: String::new(),
            ind: 0,
            nums: Vec::new(),
            anys: Vec::new(),
            in_loop: false,
            user_calls: false,
            fuel: 0,
            hot: false,
            floats: false,
            iters: 1,
            max_trips: 300,
            next_id: 0,
        }
    }

    /// 0..n; 0 once the input is used up, so variant 0 must be a leaf.
    pub(crate) fn pick(&mut self, n: u32) -> u32 {
        self.u.int_in_range(0..=n - 1).unwrap_or(0)
    }

    pub(crate) fn one<'s>(&mut self, xs: &[&'s str]) -> &'s str {
        xs[self.pick(xs.len() as u32) as usize]
    }

    pub(crate) fn line(&mut self, s: &str) {
        for _ in 0..self.ind {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    pub(crate) fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub(crate) fn loop_var(&mut self) -> Option<String> {
        if self.nums.is_empty() {
            return None;
        }
        let i = self.pick(self.nums.len() as u32) as usize;
        Some(self.nums[i].clone())
    }

    pub(crate) fn num_var(&mut self) -> String {
        let fixed: &[&str] = if self.floats {
            &["n1", "n2", "n3", "f1", "f2"]
        } else {
            &["n1", "n2", "n3"]
        };
        let i = self.pick((fixed.len() + self.nums.len()) as u32) as usize;
        match fixed.get(i) {
            Some(v) => (*v).to_string(),
            None => self.nums[i - fixed.len()].clone(),
        }
    }

    pub(crate) fn special_num(&mut self) -> String {
        let floats = [
            "0.5", "-0.0", "(1/0)", "(-1/0)", "(0/0)", "1e308", "2^53", "3.0", "-2.5", "1e-310",
        ];
        let ints = [
            "0", "-1", "math.maxinteger", "math.mininteger", "2147483647", "0x7fffffff", "7",
        ];
        if self.ints && self.pick(2) == 0 {
            self.one(&ints).to_string()
        } else if self.pick(3) == 0 {
            self.one(&["0", "-1", "7", "2^53"]).to_string()
        } else {
            self.one(&floats).to_string()
        }
    }

    pub(crate) fn num(&mut self, d: u32) -> String {
        if self.hot && self.pick(8) != 0 {
            return self.hnum(d);
        }
        let n = if d >= MAX_DEPTH { 4 } else { 22 };
        match self.pick(n) {
            0 => format!("{}", self.pick(12) as i32 - 2),
            1 => self.num_var(),
            2 => self.special_num(),
            3 => self.one(&["c[1]", "#seq", "cnt", "hits", "c.n"]).to_string(),
            4 => {
                let op = self.one(&["+", "-", "*", "+", "-"]);
                format!("({} {op} {})", self.num(d + 1), self.num(d + 1))
            }
            5 => format!("(- {})", self.num(d + 1)),
            6 => {
                let (l, r) = (self.num(d + 1), self.num(d + 1));
                match (self.ints, self.pick(2)) {
                    (true, 0) => format!("({l} // {r})"),
                    (false, 0) => format!("math.floor({l} / {r})"),
                    _ => format!("({l} % {r})"),
                }
            }
            7 => {
                let e = self.one(&["0", "1", "2", "0.5", "-1"]);
                format!("(({}) ^ {e})", self.num(d + 1))
            }
            // integer-only operators error on most generated floats, so
            // they mostly take an integer-valued left operand
            8 if self.ints && self.pick(3) == 0 => {
                let op = self.one(&["&", "|", "~", "<<", ">>"]);
                format!("({} {op} {})", self.num(d + 1), self.num(d + 1))
            }
            8 if self.ints => {
                let op = self.one(&["&", "|", "~", "<<", ">>"]);
                let l = self.one(&["c[1]", "#seq", "cnt", "hits", "#s"]);
                format!("({l} {op} {})", self.pick(9))
            }
            9 => self.math_call(d),
            10 => format!("(tonumber({}) or 1)", self.any(d + 1)),
            11 => format!("{}[{}]", self.obj(d + 1), self.key(d + 1)),
            12 => format!("(tonumber({}[{}]) or 0)", self.obj(d + 1), self.key(d + 1)),
            13 => format!("({} and {} or {})", self.cond(d + 1), self.num(d + 1), self.num(d + 1)),
            14 => self.user_call(d),
            15 => format!("#{}", self.str_e(d + 1)),
            16 => match self.pick(3) {
                0 => format!("(M + {})", self.num(d + 1)),
                1 => format!("({} + M)", self.num(d + 1)),
                _ => "(-M)".to_string(),
            },
            17 => self.one(&["\"10\"", "\"0x10\"", "\" 3 \"", "\"1e2\"", "\"-0\""]).to_string(),
            18 => self.one(&["#M", "#arr", "#c"]).to_string(),
            19 => format!("select('#', va({}, {}))", self.num(d + 1), self.any(d + 1)),
            20 => format!("({} / {})", self.num(d + 1), self.num(d + 1)),
            _ if self.ints => format!("(math.tointeger({}) or 0)", self.num(d + 1)),
            _ => format!("{}", self.pick(100)),
        }
    }

    pub(crate) fn math_call(&mut self, d: u32) -> String {
        let a = self.num(d + 1);
        match self.pick(8) {
            0 => format!("math.floor({a})"),
            1 => format!("math.abs({a})"),
            2 => format!("math.max({a}, {})", self.num(d + 1)),
            3 => format!("math.min({a}, {})", self.num(d + 1)),
            4 => format!("math.fmod({a}, {})", self.num(d + 1)),
            5 => format!("math.sqrt({a})"),
            6 => format!("math.ceil({a})"),
            _ => "math.huge".to_string(),
        }
    }

    pub(crate) fn user_call(&mut self, d: u32) -> String {
        if !self.user_calls {
            return format!("(M({}))", self.num(d + 1));
        }
        match self.pick(7) {
            6 => format!("hk({}, {})", self.num(d + 1), self.num(d + 1)),
            0 => format!("f({}, {})", self.num(d + 1), self.any(d + 1)),
            1 => format!("g({})", self.num(d + 1)),
            2 => format!("rec({}, {})", self.pick(9), self.num(d + 1)),
            3 => format!("obj:m({})", self.num(d + 1)),
            4 => format!("(va({}, {}))", self.num(d + 1), self.any(d + 1)),
            _ => format!("M({})", self.any(d + 1)),
        }
    }

    pub(crate) fn str_e(&mut self, d: u32) -> String {
        let n = if d >= MAX_DEPTH { 2 } else { 13 };
        match self.pick(n) {
            0 => self
                .one(&["\"a\"", "\"\"", "\"10\"", "\"0x1F\"", "\" 7 \"", "\"1e2\"", "\"abc\""])
                .to_string(),
            1 => "s".to_string(),
            2 => format!("tostring({})", self.num(d + 1)),
            3 => format!("({} .. {})", self.str_e(d + 1), self.str_e(d + 1)),
            4 => format!("({} .. {})", self.str_e(d + 1), self.num(d + 1)),
            5 => {
                let (i, j) = (self.pick(7) as i32 - 3, self.pick(7) as i32 - 3);
                format!("string.sub({}, {i}, {j})", self.str_e(d + 1))
            }
            6 => {
                let f = self.one(&["%d", "%.3f", "%g", "%5.1f", "%g", "%.14g", "%s"]);
                format!("string.format(\"{f}\", {})", self.num(d + 1))
            }
            7 => format!("string.rep({}, {})", self.str_e(d + 1), self.pick(4)),
            8 => {
                let f = self.one(&["upper", "lower", "reverse"]);
                format!("string.{f}({})", self.str_e(d + 1))
            }
            9 => match self.pick(2) {
                0 => format!("(M .. {})", self.str_e(d + 1)),
                _ => format!("({} .. M)", self.str_e(d + 1)),
            },
            10 => format!("type({})", self.any(d + 1)),
            11 => format!("table.concat({{{}, {}}}, \",\")", self.str_e(d + 1), self.num(d + 1)),
            _ => format!("tostring({})", self.cond(d + 1)),
        }
    }

    pub(crate) fn any(&mut self, d: u32) -> String {
        let n = if d >= MAX_DEPTH { 2 } else { 11 };
        match self.pick(n) {
            0 => self.one(&["nil", "true", "false"]).to_string(),
            1 => match self.anys.len() {
                0 => "x".to_string(),
                k => {
                    let i = self.pick(k as u32 + 1) as usize;
                    self.anys.get(i).cloned().unwrap_or_else(|| "x".to_string())
                }
            },
            2 => self.num(d + 1),
            3 => self.str_e(d + 1),
            4 => self.obj(d + 1),
            5 => format!("{}[{}]", self.obj(d + 1), self.key(d + 1)),
            6 => self.cond(d + 1),
            7 => format!("{{va({}, {})}}", self.num(d + 1), self.any(d + 1)),
            8 => format!("function(q) return q + {} end", self.pick(5)),
            9 => format!("({} and {} or {})", self.cond(d + 1), self.any(d + 1), self.any(d + 1)),
            _ => self.user_call(d),
        }
    }

    pub(crate) fn obj(&mut self, d: u32) -> String {
        let n = if d >= MAX_DEPTH { 5 } else { 7 };
        match self.pick(n) {
            0 => "t".to_string(),
            1 => "M".to_string(),
            2 => "P".to_string(),
            3 => "o".to_string(),
            4 => match self.loop_var() {
                Some(v) => format!("objs[math.floor({v}) % 3 + 1]"),
                None => "M2".to_string(),
            },
            5 => format!("({} and M or t)", self.cond(d + 1)),
            _ => "obj".to_string(),
        }
    }

    pub(crate) fn key(&mut self, d: u32) -> String {
        match self.pick(10) {
            0 => "\"x\"".to_string(),
            1 => "\"y\"".to_string(),
            2 => "1".to_string(),
            3 => "2".to_string(),
            4 => "3".to_string(),
            5 => self.loop_var().unwrap_or_else(|| "\"z\"".to_string()),
            6 => "2.0".to_string(),
            7 => "\"v\"".to_string(),
            8 if d < MAX_DEPTH => self.num(d + 1),
            _ if d < MAX_DEPTH => self.str_e(d + 1),
            _ => "\"w\"".to_string(),
        }
    }

    pub(crate) fn cond(&mut self, d: u32) -> String {
        if self.hot && self.pick(8) != 0 {
            return self.hcond(d);
        }
        let n = if d >= MAX_DEPTH { 2 } else { 10 };
        match self.pick(n) {
            0 => self.one(&["true", "false"]).to_string(),
            1 => {
                let op = self.one(&["<", "<=", ">", ">=", "==", "~="]);
                let (l, r) = if d >= MAX_DEPTH {
                    (self.num_var(), self.num_var())
                } else {
                    (self.num(d + 1), self.num(d + 1))
                };
                format!("({l} {op} {r})")
            }
            2 => match self.loop_var() {
                Some(v) => match self.pick(3) {
                    0 => format!("({v} % {} == 0)", self.pick(7) + 2),
                    1 => format!("({v} > {})", self.pick(200)),
                    _ => format!("({v} == {})", self.pick(100)),
                },
                None => format!("(n1 > {})", self.pick(10)),
            },
            3 => {
                let op = self.one(&["<", "==", "<="]);
                format!("({} {op} {})", self.str_e(d + 1), self.str_e(d + 1))
            }
            4 => format!("({} == {})", self.obj(d + 1), self.obj(d + 1)),
            5 => self.one(&["(M < M2)", "(M <= M2)", "(M2 < M)", "(M == M2)"]).to_string(),
            6 => format!("(not {})", self.any(d + 1)),
            7 => {
                let op = self.one(&["and", "or"]);
                format!("({} {op} {})", self.cond(d + 1), self.cond(d + 1))
            }
            8 => {
                let e = self.num(d + 1);
                format!("({e} ~= {e})")
            }
            _ => self
                .one(&["(x == nil)", "(type(x) == \"number\")", "rawequal(o, t)"])
                .to_string(),
        }
    }
}
