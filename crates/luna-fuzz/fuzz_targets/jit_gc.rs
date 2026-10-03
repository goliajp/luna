//! A collection while a trace holds new objects only in its own values.
//! A trace keeps the tables it creates in registers of the compiled code
//! until it stores them somewhere the collector can see; an operation in
//! between that can collect (a concatenation, a native iterator, a
//! metamethod call) must not free them. The kernel builds a table per
//! iteration with such an operation in the middle, stores it, and checks
//! every table afterwards; the GC pause is lowered and the kernel runs
//! several times so collections land inside the trace. Nothing here
//! recurses, so the generator's allocation stacks stay a fixed set.

use super::jit_program::Gen;

impl Gen<'_, '_> {
    /// Something that can run the collector, with `i` in scope.
    fn gc_op(&mut self) -> &'static str {
        self.one(&[
            "local s2 = \"b\" .. i",
            "local s2 = tostring(i) .. \":\" .. i",
            "local s2 = string.rep(\"ab\", 3) .. i",
            "for _, v in pairs(src) do cnt = cnt + 1 end",
            "for _, v in ipairs(src) do cnt = cnt + 1 end",
            "local m = M[i]",
            "local m = M .. \"x\"",
            "local m = #M",
        ])
    }

    /// `do local function gk(r) ... end for r = 1, R do ... end end`
    pub(crate) fn gc_kernel(&mut self) {
        let n = [40, 100, 200, 300][self.pick(4) as usize];
        let reps = [3, 10, 25][self.pick(3) as usize];
        let pause = self.one(&["0", "10", "50", "100"]);
        // 5.5 sets the pause through `param`, 5.1–5.4 through `setpause`
        self.line(&format!(
            "if _VERSION == \"Lua 5.5\" then collectgarbage(\"param\", \"pause\", {pause}) \
             else collectgarbage(\"setpause\", {pause}) end"
        ));
        self.line("do");
        self.ind += 1;
        self.line("local src = {1, 2, 3, x = 1}");
        self.line("local function gk(r)");
        self.line("  local bs = {}");
        self.line(&format!("  for i = 1, {n} do"));
        let op = self.gc_op();
        match self.pick(3) {
            // the operation inside the constructor, between the new table
            // and its last field
            0 => {
                let field = match op {
                    "local s2 = \"b\" .. i" => "\"b\" .. i",
                    "local s2 = tostring(i) .. \":\" .. i" => "tostring(i) .. \":\" .. i",
                    _ => "s .. i",
                };
                self.line(&format!("    bs[i] = {{tokens = i, name = {field}}}"));
            }
            // the new table in a local across the operation
            1 => {
                self.line("    local nt = {tokens = i}");
                self.line(&format!("    {op}"));
                self.line("    nt.name = i");
                self.line("    bs[i] = nt");
            }
            // two new tables, one inside the other
            _ => {
                self.line("    local nt = {tokens = i, sub = {i}}");
                self.line(&format!("    {op}"));
                self.line("    bs[i] = nt");
            }
        }
        self.line("  end");
        self.line(&format!("  for i = 1, {n} do"));
        self.line("    local b = bs[i]");
        self.line(
            "    if type(b) ~= \"table\" or b.tokens ~= i then \
             error(\"bad \" .. i .. \" \" .. type(b) .. \" \" .. tostring(b and b.tokens)) end",
        );
        self.line("  end");
        self.line("  return #bs");
        self.line("end");
        self.line(&format!("for r = 1, {reps} do psum = psum + gk(r) end"));
        self.ind -= 1;
        self.line("end");
    }
}
