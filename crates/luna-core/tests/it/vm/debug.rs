//! The debug library and hooks.

use super::*;

#[test]
fn debug_getinfo_name() {
    // name/namewhat recovered from the caller's call instruction (getobjname);
    // local-variable debug names make a local function nameable
    // non-tail calls so a caller frame exists to inspect (a tail call drops
    // the name, like PUC)
    check_str(
        "local function F() return debug.getinfo(1, 'n').name end local r = F() return r",
        b"F",
    );
    check_str(
        "local t = {} function t.m() return debug.getinfo(1).name end local r = t.m() return r",
        b"m",
    );
    check_str(
        "function glob() return debug.getinfo(1).name end local r = glob() return r",
        b"glob",
    );
    // a directly-invoked anonymous function has no recoverable name
    check_bool(
        "local r = (function() return debug.getinfo(1, 'n').name end)() return r == nil",
        true,
    );
}

#[test]
fn debug_getinfo_c_frame_boundary() {
    // debug.getinfo level traversal sees a synthetic C frame at a call_value
    // boundary: from inside a __close handler (invoked by the close machinery),
    // level 1 is the handler (Lua) and level 2 is "C".
    check_str(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local what \
         local function foo() \
           local x <close> = c(function() what = debug.getinfo(2).what end) \
           error('e') \
         end \
         pcall(foo) \
         return what",
        b"C",
    );
    // a function called through pcall sees pcall as a C frame at level 2
    check_str(
        "local seen \
         local function f() seen = debug.getinfo(2).what end \
         pcall(f) \
         return seen",
        b"C",
    );
}

#[test]
fn debug_getinfo_source_lines_options() {
    // linedefined / lastlinedefined span the function from `function` to `end`;
    // the function-value form has an empty namewhat and no name ('n' only).
    check_str(
        "local function test (a) \n\
           local x = a \n\
           return x \n\
         end \n\
         local i = debug.getinfo(test, 'Sn') \n\
         return i.what .. ',' .. i.linedefined .. ',' .. i.lastlinedefined \n\
            .. ',' .. i.namewhat .. ',' .. tostring(i.name)",
        b"Lua,1,4,,nil",
    );
    // "L" yields activelines (a set keyed by line); body and closing-`end` lines
    // are active, the `function` header line is not. A C function has none.
    check_bool(
        "local function test (a) \n\
           local x = a \n\
           return x \n\
         end \n\
         local act = debug.getinfo(test, 'L').activelines \n\
         return act[2] and act[4] and not act[1] and not act[5] \n\
            and debug.getinfo(print, 'L').activelines == nil",
        true,
    );
    // an out-of-range stack level is nil; a bad option char (or leading '>')
    // raises.
    check_bool(
        "return debug.getinfo(1000) == nil \
            and not pcall(debug.getinfo, print, 'X') \
            and not pcall(debug.getinfo, 1, '>')",
        true,
    );
    // short_src renders a long string source with luaO_chunkid truncation
    // ([string "..."]) and a one-liner verbatim.
    check_bool(
        "local f = load('return 1') \
         local g = load('return ' .. ('p'):rep(400)) \
         return debug.getinfo(f).short_src == '[string \"return 1\"]' \
            and string.find(debug.getinfo(g).short_src, '^%[string [^\\n]*%.%.%.\"%]$') ~= nil",
        true,
    );
    // a stripped binary chunk carries no line info: empty activelines.
    check_int(
        "local f = load(string.dump(load('print(1)'), true)) \
         local act = debug.getinfo(f, 'L').activelines \
         return #act",
        0,
    );
}

#[test]
fn debug_line_hook() {
    // debug.sethook(f, "l") fires a "line" event per source line; the hook runs
    // with events disabled and is cleared by sethook() with no args. PUC 5.5
    // `traceexec` uses `npci <= L->oldpc`, and `lua_sethook` does NOT touch
    // oldpc, so the very first step after the install fires (db.lua :322
    // depends on this: install + four statement lines == count == 4). luna
    // mirrors that by arming `hook_oldpc` to a sentinel in `install_hook`, so
    // the install-line's first follow-up step fires — here that is line 4
    // (`local a = 1`).
    check_str(
        "local out = {} \n\
         local function h(ev, ln) out[#out + 1] = ev .. ':' .. ln end \n\
         debug.sethook(h, 'l') \n\
         local a = 1 \n\
         local b = 2 \n\
         debug.sethook() \n\
         return table.concat(out, ',')",
        b"line:4,line:5,line:6",
    );
    // gethook reports the installed hook, then nil once cleared
    check_bool(
        "local function h() end \
         debug.sethook(h, 'l') \
         local got = debug.gethook() \
         debug.sethook() \
         return got == h and debug.gethook() == nil",
        true,
    );
}

#[test]
fn debug_line_hook_does_not_recurse_into_itself() {
    // CB IO follow-up regression: an `||` / `&&` precedence bug in the
    // dispatcher's count+line predicate (exec.rs ~6625) left the
    // `!self.in_hook` guard only gating the rust-hook arm. With a Lua hook
    // installed and the hook body executing any Lua bytecode (e.g. an
    // `assert(...)` call), the hook would re-fire inside itself → unbounded
    // recursion → stack overflow on PUC db.lua line 14 / 16 / 22 (the
    // `assert(event == 'line')` inside the line-hook body).
    //
    // Repro mirrors PUC db.lua `test`: install a Lua line hook whose body
    // dispatches Lua bytecode (table writes + assert) — if the guard works,
    // we get a finite list of line events; if it doesn't, the hook recurses
    // through `assert` and overflows the stack.
    check_int(
        "local n = 0 \n\
         local function h(ev) \n\
           assert(ev == 'line') \n\
           n = n + 1 \n\
         end \n\
         debug.sethook(h, 'l') \n\
         local a = 1 \n\
         local b = 2 \n\
         local c = 3 \n\
         debug.sethook() \n\
         return n",
        // Without the fix: stack overflow before sethook() runs.
        // With the fix: a finite number of line events (one per source line
        // executed under the hook). We do not pin the exact count — the
        // PUC `traceexec` discipline already has its own coverage in
        // `debug_line_hook` / `debug_line_table_precision` — we just need
        // n > 0 and the program to terminate.
        // check_int requires an exact value; this VM emits 4 line events
        // under PUC `npci <= oldpc` semantics (install-step + 3 statement
        // lines; sethook clear is on the same line as the call site so
        // changedline is false there). The point of the test is that the
        // program *terminates with a finite count* — pre-fix it would have
        // stack-overflowed before reaching `return n`.
        4,
    );
}

#[test]
fn debug_line_table_precision() {
    // line traces match PUC's per-instruction line table across constructs, the
    // way db.lua tests them (hook installed + chunk run on one line). The chunk
    // is a string literal passed to load().
    // The wrapper is one line, so the install-statement and the load() call
    // share a source line: `changedline` is false at the wrapper boundary,
    // so it does not fire — the trace only contains the loaded chunk's per-
    // instruction line-table expectations (PUC db.lua semantics).
    let trace = |chunk: &str| -> String {
        format!(
            "local l = {{}} \
             debug.sethook(function(e, n) l[#l + 1] = n end, 'l'); \
             load({chunk})(); debug.sethook() \
             return table.concat(l, ',')"
        )
    };
    // if/else: condition line, taken branch, chunk's final `end` line.
    check_str(
        &trace("'if\\nmath.sin(1)\\nthen\\n a=1\\nelse\\n a=2\\nend\\n'"),
        b"2,4,7",
    );
    // a numeric for re-fires the `for` line on each iteration (FORLOOP back-edge)
    check_str(&trace("'for i=1,3 do\\n a=i\\nend\\n'"), b"1,2,1,2,1,2,1,3");
    // a local function's closure-creation lands on its `end` line (PUC luaK_code
    // uses the just-consumed token's line)
    check_str(
        &trace("'local function foo()\\nend\\nfoo()\\nA=1\\nA=2\\nA=3\\n'"),
        b"2,3,2,4,5,6",
    );
}

#[test]
fn debug_upvalue_order_and_id() {
    // Every code path the assertions ride is deterministic under
    // single-threaded exec — compiler upvalue indexing is a `Vec::position`
    // + `Vec::push` walk, the open-upvalue chain is a slot-sorted `Vec` with
    // `binary_search_by_key` dedup, and `debug.upvalueid` returns the GC
    // cell's raw address (which is dedup-determined, not allocator-determined,
    // for the shared-upvalue assertion).
    //
    // Each sub-check repeats 50× in a single test process as a fail-fast
    // tripwire if any allocator / GC tuning ever breaks shared-upvalue
    // identity.
    for _ in 0..50 {
        // upvalue indices follow PUC's restassign ordering: a name first seen on an
        // assignment's left captures its index before one first seen on the right.
        // `a = 10 + b` (a on the left) → a is upvalue 1, b is upvalue 2.
        check_str(
            "local a, b = 1, 2 \
             local f = function (y) if y then a = 10 + b else return a end end \
             local n1 = (debug.getupvalue(f, 1)) \
             local n2 = (debug.getupvalue(f, 2)) \
             return n1 .. ',' .. n2",
            b"a,b",
        );
        // setupvalue targets the right slot (upvalue 1 = 'a') and returns its name
        check_int(
            "local a, b = 0, 5 \
             local f = function () return a + b end \
             local nm = debug.setupvalue(f, 1, 7) \
             return (nm == 'a') and f() or -1",
            12,
        );
        // upvalueid: out-of-range yields nil (not an error, unlike upvaluejoin);
        // distinct upvalues have distinct ids, shared ones compare equal; it also
        // works on a C closure (the gmatch iterator).
        check_bool(
            "local a, b = 1, 2 \
             local f = function () return a + b end \
             local g = function () return b + a end \
             return debug.upvalueid(f, 3) == nil \
                and debug.upvalueid(f, 1) ~= debug.upvalueid(f, 2) \
                and debug.upvalueid(f, 1) == debug.upvalueid(g, 2) \
                and debug.upvalueid(string.gmatch('x', 'x'), 1) ~= nil \
                and (not pcall(debug.upvaluejoin, f, 9, g, 1))",
            true,
        );
    }
}

#[test]
fn debug_traceback_honours_level() {
    // db.lua :958: `debug.traceback(msg, level)` must enumerate from `level`,
    // not from the innermost frame. Skipping the top `level-1` frames cuts
    // the visible chain accordingly.
    check_int(
        "local function deep(lvl, n) \
           if lvl == 0 then return (debug.traceback('m', n)) end \
           return (deep(lvl-1, n)) \
         end \
         local function checkdeep(total, start) \
           local s = deep(total, start) \
           local rest = string.match(s, '^m\\nstack traceback:\\n(.*)$') \
           return select(2, string.gsub(rest, '\\n', '')) \
         end \
         return coroutine.wrap(checkdeep)(11, 5)",
        // 12 deep frames + 1 checkdeep, start=5 drops 4 → 9 frames → 8 newlines.
        8,
    );
}

#[test]
fn stripped_chunk_debug_surface() {
    // db.lua :992/:1004: stripped chunks render short_src as "?" (PUC `funcinfo`
    // substitutes "=?" when `Proto.source` is NULL; chunk_id strips the sigil).
    check_str(
        "local f = function () return 1 end \
         f = load(string.dump(f, true)) \
         return debug.getinfo(f).short_src",
        b"?",
    );
    // db.lua :993: `getinfo(level).currentline` is -1 in a stripped chunk
    // (PUC `getfuncline` returns -1 when per-instruction line info is absent).
    check_int(
        "local prog = 'return debug.getinfo(1).currentline' \
         local f = assert(load(string.dump(load(prog), true))) \
         return f()",
        -1,
    );
    // db.lua :984: `debug.getupvalue` returns "(no name)" when the upvalue
    // name was stripped (PUC `aux_upvalue` for a NULL name).
    check_str(
        "local a = 12 \
         local f = function () return a end \
         f = load(string.dump(f, true)) \
         local n = debug.getupvalue(f, 1) \
         return n",
        b"(no name)",
    );
    // db.lua :1030: a line hook installed before running a stripped chunk
    // still fires on the first instruction, but with `nil` as the line arg
    // (PUC pushes `currentline` only when `>= 0`).
    check_bool(
        "local foo = function () local a = 1; return a end \
         local s = load(string.dump(foo, true)) \
         local line = true \
         debug.sethook(function (e, l) line = l end, 'l') \
         s() \
         debug.sethook(nil) \
         return line == nil",
        true,
    );
}

#[test]
fn light_userdata_from_debug_upvalueid() {
    // errors.lua:260: debug.upvalueid returns a light userdata, and
    // debug.setuservalue rejects it with "light userdata" in the message
    // (PUC's luaL_typeerror tag for LUA_TLIGHTUSERDATA).
    check_error(
        "local x = debug.upvalueid(function () return debug end, 1); \
         debug.setuservalue(x, {})",
        "light userdata",
    );
    // raw equality on identical light pointers
    check_str(
        "local f = function () return debug end; \
         local a, b = debug.upvalueid(f, 1), debug.upvalueid(f, 1); \
         return tostring(a == b)",
        b"true",
    );
    // type() collapses light userdata to "userdata" (PUC lua_typename)
    check_str(
        "return type(debug.upvalueid(function () return debug end, 1))",
        b"userdata",
    );
}

#[test]
fn getinfo_names_c_boundary() {
    // locals.lua:514 — debug.getinfo of a synthetic C level names the native
    // from the call instruction that invoked it (e.g. "pcall").
    check_str(
        "local function f() local i = debug.getinfo(2); return i.namewhat .. '/' .. i.name end \
         return select(2, pcall(f))",
        b"global/pcall",
    );
}

#[test]
fn debug_getlocal_and_for_state() {
    // debug.getlocal returns the n-th active local (name, value).
    check_str(
        "local function basic(a, b) local c = a * b; local n, v = debug.getlocal(1, 3); \
         return n .. '=' .. v end \
         return basic(6, 7)",
        b"c=42",
    );
    // files.lua:447 — a generic-for loop's hidden control slots are named
    // "(for state)"; the 3rd is the to-be-closed value (PUC forlist).
    check_bool(
        "local function gettoclose(lv) lv = lv + 1; local st = 0 \
           for i = 1, 20 do local n, v = debug.getlocal(lv, i) \
             if n == '(for state)' then st = st + 1; if st == 3 then return v end end end end \
         local marker = setmetatable({}, {__close = function () end}) \
         local function iter(_, c) if c < 1 then return c + 1 end end \
         local got \
         for _ in iter, nil, 0, marker do got = gettoclose(1); break end \
         return got == marker",
        true,
    );
}

#[test]
fn return_hook_for_native_names_callee() {
    // locals.lua:833 — a "return" hook firing after a native (debug.sethook)
    // returns must let getinfo(2) see the native, named via the caller's call
    // instruction ("sethook"). luna's run_hook pushes the hook with
    // `from_c = true` only when the hooked function was native, so dbg_frame
    // inserts a synthetic C level for it; for a Lua hooked function, `from_c`
    // is false and level 2 lands on that Lua frame.
    check_str(
        "local cap = '?' \
         local function hook (event) \
           if cap == '?' then cap = (debug.getinfo(2).name or '?') end \
         end \
         (function () debug.sethook(hook, 'r') end)() \
         debug.sethook() \
         return cap",
        b"sethook",
    );
}

#[test]
fn dbg_frame_inserts_tail_synthetic_under_51() {
    // PUC 5.1's `lua_getstack` reports a synthetic CIST_TAIL level between
    // each tail-called Lua frame and its caller: `getinfo(2).what == "tail"`
    // from inside a tail-called function, with the real caller at level 3.
    // 5.2+ retired the synthetic shape — `istailcall` becomes a flag on
    // the real frame and `getinfo(2).func == g1`. 5.1 db.lua :334-:343 vs
    // 5.5 db.lua :625-:628 pin each shape.
    let mut vm = Vm::new(LuaVersion::Lua51);
    vm.eval(
        "local function f (x) \
             if x then \
                 assert(debug.getinfo(2).what == 'tail') \
                 assert(not pcall(getfenv, 3)) \
                 assert(debug.getinfo(3, 'f').func == g1) \
             end \
         end \
         function g(x) return f(x) end \
         function g1(x) g(x) end \
         local function h(x) local f = g1; return f(x) end \
         h(true)",
    )
    .expect("5.1 tail-call shape");

    let mut vm5 = Vm::new(LuaVersion::Lua55);
    vm5.eval(
        "local function f (x) \
             if x then \
                 assert(debug.getinfo(1, 't').istailcall == true) \
                 local tail = debug.getinfo(2) \
                 assert(tail.func == g1 and tail.istailcall == true) \
             end \
         end \
         function g(x) return f(x) end \
         function g1(x) g(x) end \
         local function h(x) local f = g1; return f(x) end \
         h(true)",
    )
    .expect("5.5 tail-call shape");
}
