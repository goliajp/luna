//! Short loops inside a short loop, in a function called many times. Each
//! loop head is counted on its own, so the inner loop can get hot first
//! and start a trace that leaves it, runs the rest of the outer body and
//! takes the outer loop's back-edge: a trace that treats that back-edge as
//! its own skips whatever the outer body does before the inner loop (the
//! reset of the inner counter here). Few trips per loop keep both loops
//! close to the hot threshold, where the order they get hot in varies.

use super::jit_program::Gen;

impl Gen<'_, '_> {
    /// `do local function nk(a) <outer> <inner> ... end end <calls> end`
    pub(crate) fn nested_kernel(&mut self) {
        let outer_trips = self.pick(4) + 2;
        let inner_trips = self.pick(2) + 1;
        let calls = [40, 200, 600][self.pick(3) as usize];
        self.line("do");
        self.ind += 1;
        self.line("local list = {}");
        self.line(&format!("for q = 1, {outer_trips} do list[q] = q end"));
        self.line("local function nk(a)");
        self.ind += 1;
        self.line("local s = 0");
        let outer_close = match self.pick(3) {
            0 => {
                self.line(&format!("for i = 1, {outer_trips} do"));
                "end"
            }
            1 => {
                self.line("for _, i in ipairs(list) do");
                "end"
            }
            _ => {
                self.line("local i = 0");
                self.line("repeat i = i + 1");
                "until i >= #list"
            }
        };
        self.ind += 1;
        if self.pick(2) == 0 {
            self.line("s = s + i");
        }
        let inner_close = match self.pick(3) {
            0 => {
                self.line("local w = 0");
                self.line(&format!("while w < {inner_trips} do w = w + 1"));
                "end".to_string()
            }
            1 => {
                self.line("local w = 0");
                self.line("repeat w = w + 1");
                format!("until w >= {inner_trips}")
            }
            _ => {
                self.line(&format!("for w = 1, {inner_trips} do"));
                "end".to_string()
            }
        };
        self.ind += 1;
        let step = match self.pick(4) {
            0 => "s = s + 1",
            1 => "s = s + w",
            2 => "s = s + a % 3",
            _ => "s = s + i * w",
        };
        self.line(step);
        if self.pick(3) == 0 {
            self.line("if a % 7 == 0 then s = s - 1 end");
        }
        self.ind -= 1;
        self.line(&inner_close);
        self.ind -= 1;
        self.line(outer_close);
        self.line("return s");
        self.ind -= 1;
        self.line("end");
        self.line("local tot = 0");
        self.line(&format!("for a = 1, {calls} do tot = tot + nk(a) end"));
        self.line("print(\"nk\", tot)");
        self.ind -= 1;
        self.line("end");
    }
}
