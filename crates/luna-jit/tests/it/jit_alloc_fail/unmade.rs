//! What an exit could not make, when the dispatcher cannot make it either:
//! the memory error, as the interpreter raises it.
//!
//! Where it is raised differs from the interpreter only where the trace
//! made the object later than the interpreter does. A frame is pushed
//! when the call into it is made, and the error is that call's in both. A
//! sunk table or an accumulated string is made at the exit, after what the
//! trace did past the `NewTable` or `CONCAT` the interpreter fails at: the
//! count below is one more than the interpreter's for a sunk table, and
//! the passes since the trace was entered have run for a string.

use super::*;

/// Refuses the `from`th growth of `kind` compiled code asks for, and the
/// `more` growths of that kind after it wherever they are made.
struct Window {
    kind: BlockKind,
    from: usize,
    more: usize,
    seen: Arc<AtomicUsize>,
    left: usize,
}

impl MemoryPolicy for Window {
    fn allow(&mut self, _old: usize, _new: usize, kind: BlockKind, _in_use: usize) -> bool {
        if kind != self.kind {
            return true;
        }
        if self.left > 0 {
            self.left -= 1;
            return false;
        }
        if !in_compiled_code() {
            return true;
        }
        let n = self.seen.fetch_add(1, Relaxed) + 1;
        if n != self.from {
            return true;
        }
        self.left = self.more;
        false
    }
}

/// The end of a chunk whose `all` runs the loops, counting the passes in
/// `cnt[1]` and noting the pass in `cnt[2]`.
const REPORT: &str = "local ok, e = pcall(all) \
     return tostring(ok) .. ':' .. tostring(e) .. ':' .. cnt[1] .. ':' .. cnt[2]";

/// The tries the dispatcher makes: one, and one more after an emergency
/// collection from 5.2 on.
fn tries(v: LuaVersion) -> usize {
    if v == LuaVersion::Lua51 { 1 } else { 2 }
}

/// The result, how many growths of `kind` compiled code asked for, and
/// the frames, sunk tables and strings the dispatcher made or tried to.
fn run(v: LuaVersion, kind: BlockKind, from: usize, src: &str) -> (String, usize, [u64; 3]) {
    let seen = Arc::new(AtomicUsize::new(0));
    let p = Window {
        kind,
        from,
        more: tries(v),
        seen: seen.clone(),
        left: 0,
    };
    let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(p)));
    luna_jit::install_default_jit(&mut vm);
    // the first coroutine, not the first pass, compiles the frames' trace
    let hot = if src.contains("coroutine.wrap(run)") {
        64
    } else {
        1
    };
    vm.set_trace_hot_threshold(hot);
    vm.set_call_hot_threshold(hot);
    vm.set_jit_enabled(false);
    let r = report(&mut vm, src);
    assert!(
        vm.jit().pending_err.is_none(),
        "{v:?} #{from}: a deopt left parked"
    );
    let c = vm.jit_unmade_counts();
    (r, seen.load(Relaxed), [c.frames, c.sunk, c.accum])
}

/// The string the chunk returns, or the error it raised.
fn report(vm: &mut Vm, src: &str) -> String {
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

/// The count and the pass of a run that raised the memory error, `None`
/// for one that did not; `want` is the program's result otherwise.
fn raised(r: &str, want: &str, ctx: &str) -> Option<(i64, i64)> {
    if r == want {
        return None;
    }
    // a coroutine's error comes out of `coroutine.wrap` with the position
    // of its call in front
    let body = r
        .strip_prefix("false:")
        .unwrap_or_else(|| panic!("{ctx}: {r}"));
    let mut it = body.rsplitn(3, ':');
    let (l, c, msg) = (it.next(), it.next(), it.next());
    assert!(
        msg.is_some_and(|m| m.ends_with("not enough memory")),
        "{ctx}: {r}"
    );
    Some((c.unwrap().parse().unwrap(), l.unwrap().parse().unwrap()))
}

/// Runs `src` with each of up to `max` of the growths of `kind` its traces
/// make refused, and the dispatcher's tries after it; checks each run that
/// raised with `check(count, pass)`. How many raised once the dispatcher
/// had tried to make what counter `made` counts.
fn sweep(
    v: LuaVersion,
    src: &str,
    (kind, made): (BlockKind, usize),
    check: impl Fn(i64, i64) -> bool,
) -> usize {
    let want = report(&mut Vm::new(v), src);
    let (_, n, _) = run(v, kind, usize::MAX, src);
    let mut errors = 0;
    for k in spread(n, 12) {
        let (r, _, m) = run(v, kind, k, src);
        let ctx = format!("{v:?} #{k}");
        if let Some((c, l)) = raised(&r, &want, &ctx) {
            assert!(check(c, l), "{ctx}: {r}");
            errors += usize::from(m[made] > 0);
        }
    }
    errors
}

#[test]
fn a_frame_the_dispatcher_cannot_push_raises_at_the_call() {
    let src = super::rerun::frames_prog(REPORT);
    // each coroutine runs 300 passes: the pass made its count and called
    // the function; only 5.1's coroutines start on a stack too short for
    // the frames
    let raised: usize = VERSIONS
        .into_iter()
        .map(|v| sweep(v, &src, (BlockKind::Other, 0), |c, l| (c - l) % 300 == 0))
        .sum();
    assert!(raised > 0, "the dispatcher raised for no frame");
}

#[test]
fn a_sunk_table_the_dispatcher_cannot_make_raises_at_the_exit() {
    let src = "local cnt = {0, 0} local function all() local s = 0 \
         for i = 1, 3000 do cnt[2] = i local p = {i, i * 2} cnt[1] = cnt[1] + 1 \
           if i % 5 == 0 then s = s + p[2] end end return s end ";
    let src = format!("{src}{REPORT}");
    // the dispatcher raises after the pass made its count; the interpreter
    // raises at the `NewTable`, before it (a refusal left to it); 5.5's
    // constructor is not sunk
    let raised: usize = VERSIONS
        .into_iter()
        .map(|v| sweep(v, &src, (BlockKind::Table, 1), |c, l| c == l || c == l - 1))
        .sum();
    assert!(raised > 0, "the dispatcher raised for no table");
}

#[test]
fn a_string_the_dispatcher_cannot_make_raises_at_the_exit() {
    let src = "local cnt = {0, 0} local function all() local piece = string.rep('x', 1024) \
         local s = '' for i = 1, 400 do cnt[2] = i cnt[1] = cnt[1] + 1 s = s .. piece end \
         return #s end ";
    let src = format!("{src}{REPORT}");
    // the passes since the trace was entered have run; 5.1 and 5.4 make
    // the strings in the interpreter
    let raised: usize = VERSIONS
        .into_iter()
        .map(|v| sweep(v, &src, (BlockKind::Str, 2), |c, l| c == l))
        .sum();
    assert!(raised > 0, "the dispatcher raised for no string");
}
