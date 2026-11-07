//! Levels compiled code hands over become interpreter frames one by one:
//! a frame that cannot be pushed raises the memory error of the call the
//! level below was making, and a message handler sees the frames the
//! interpreter would have.

use luna_jit::runtime::CallFrame;

use super::*;

/// The frame stack capacities whose growth is refused: above the depth
/// the warm-up reaches.
const CAPS: [usize; 3] = [32, 64, 128];

/// A policy refusing, once armed, the first block compiled code makes and
/// the first growth of the frame stack past `cap` frames, both tries of
/// it (before and after the emergency collection). It notes the order:
/// the block's refusal and the frames' first, counting from 1.
struct RefuseFrames {
    armed: Arc<AtomicBool>,
    cap: usize,
    seq: usize,
    table_at: Arc<AtomicUsize>,
    frames_at: Arc<AtomicUsize>,
    frames_refused: Arc<AtomicUsize>,
}

impl MemoryPolicy for RefuseFrames {
    fn allow(&mut self, old: usize, new: usize, kind: BlockKind, _in_use: usize) -> bool {
        if !self.armed.load(Relaxed) {
            return true;
        }
        self.seq += 1;
        if in_compiled_code() && old == 0 && self.table_at.load(Relaxed) == 0 {
            self.table_at.store(self.seq, Relaxed);
            return false;
        }
        let frames = self.cap * std::mem::size_of::<CallFrame>();
        if kind == BlockKind::Other
            && old == frames
            && new > old
            && self.frames_refused.load(Relaxed) < 2
        {
            if self.frames_refused.fetch_add(1, Relaxed) == 0 {
                self.frames_at.store(self.seq, Relaxed);
            }
            return false;
        }
        true
    }
}

/// Recursion `DEPTH` deep the method JIT compiles whole, making a table
/// at the bottom; the handler keeps the error, the traceback and the
/// frames' names and lines.
const DOWN: &str = "local function h(e) local l = {tostring(e), debug.traceback('', 2)} \
       for i = 2, 8 do local f = debug.getinfo(i, 'nSl') \
         if f then l[#l + 1] = tostring(f.name) .. ':' .. f.what .. ':' .. f.currentline end end \
       return table.concat(l, '|') end \
     local ok, r = xpcall(function() return (down(DEPTH)) end, h) \
     return tostring(ok) .. '|' .. (type(r) == 'table' and 'table' or r)";

/// What [`DOWN`] returns, with the method JIT on or off, `pad` tables made
/// between the warm-up and the run, and the policy's notes: the block's
/// refusal, the frames' first refusal and how many tries of the frames'
/// growth it refused.
fn down(
    v: LuaVersion,
    jit: bool,
    (cap, depth): (usize, usize),
    pad: usize,
) -> (String, usize, usize, usize) {
    let armed = Arc::new(AtomicBool::new(false));
    let (table_at, frames_at, frames_refused) = Default::default();
    let policy = RefuseFrames {
        armed: armed.clone(),
        cap,
        seq: 0,
        table_at: Arc::clone(&table_at),
        frames_at: Arc::clone(&frames_at),
        frames_refused: Arc::clone(&frames_refused),
    };
    let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(policy)));
    if jit {
        luna_jit::install_default_jit(&mut vm);
        vm.set_trace_jit_enabled(false);
        vm.set_call_hot_threshold(1);
    }
    let warm = "local function f(d) if d == 0 then return {d} end local t = f(d - 1) return t end \
                down = f for i = 1, 20 do f(4) end";
    vm.eval(warm).expect("warm");
    // the tables made after the warm-up move the collector's debt, and so
    // whether a step falls due at the bottom table: when none does,
    // compiled code makes the table itself
    vm.eval(&format!("for i = 1, {pad} do local t = {{}} end"))
        .expect("pad");
    let compiled = match vm.eval("return down").expect("down").first() {
        Some(Value::Closure(cl)) => {
            matches!(cl.proto.jit_state(), JitProtoState::Compiled { .. })
        }
        other => panic!("{v:?}: down is {other:?}"),
    };
    assert_eq!(compiled, jit, "{v:?}: compiled {compiled}");
    armed.store(true, Relaxed);
    let r = match vm.eval(&DOWN.replace("DEPTH", &depth.to_string())) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    let notes = (
        table_at.load(Relaxed),
        frames_at.load(Relaxed),
        frames_refused.load(Relaxed),
    );
    (r, notes.0, notes.1, notes.2)
}

/// The policy refuses the growth of luna's own frame stack. Compiled
/// levels push no frames, so the two runs grow it alike only where the
/// levels are handed over: the policy refused compiled code's table at the
/// bottom. There the error, the traceback and `debug.getinfo` in the
/// handler are the interpreter's. The levels compiled code enters go only
/// as far as PUC has room for call records; the interpreter makes the call
/// after them, pushing its frame. For some depths the frame past the
/// capacity is one of the levels handed over, pushed as they become frames.
/// With 20 tables made after the warm-up, compiled code makes the bottom
/// table in every dialect.
#[test]
fn a_frame_a_hand_over_cannot_push_raises_as_the_interpreter() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut resumed = Vec::new();
        for cap in CAPS {
            for depth in cap - 8..cap + 8 {
                let (want, _, _, refused) = down(v, false, (cap, depth), 20);
                if refused > 0 {
                    assert_eq!(refused, 2, "{v:?} {cap} {depth}: refused {refused}");
                    assert!(
                        want.starts_with("false|") && want.contains("not enough memory"),
                        "{v:?} {cap} {depth}: no memory error: {want}"
                    );
                }
                let (got, table_at, frames_at, _) = down(v, true, (cap, depth), 20);
                if table_at > 0 {
                    assert_eq!(got, want, "{v:?} {cap} {depth}");
                }
                // the frames grew after the levels handed over: as they
                // resumed
                if table_at > 0 && frames_at > table_at {
                    resumed.push((cap, depth));
                }
            }
        }
        assert!(
            !resumed.is_empty(),
            "{v:?}: no frame refused as levels resumed"
        );
    }
}

/// Where a collection step falls due at the bottom table, compiled code
/// leaves it to the interpreter; with no refusal to hand the levels over,
/// they run through without growing the frame stack (but for the levels
/// past PUC's room for call records, which the interpreter makes), and the
/// recursion may return its table where the interpreter, pushing a frame
/// per level, runs out of memory. What a policy sees is how the Vm allocates,
/// which compiled code changes.
#[test]
fn compiled_levels_need_no_frames_a_policy_refuses() {
    let mut differ = 0;
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for cap in CAPS {
            for depth in cap - 8..cap + 8 {
                let (want, ..) = down(v, false, (cap, depth), 200);
                let (got, table_at, ..) = down(v, true, (cap, depth), 200);
                // where they differ, the compiled run returned
                assert!(
                    got == want || (table_at == 0 && got == "true|table"),
                    "{v:?} {cap} {depth}: {got} for {want}"
                );
                differ += usize::from(got != want);
            }
        }
    }
    assert!(differ > 0, "the runs never differ");
}
