//! The frames of the calls a trace inlines: how many arguments each call
//! passes (a variable count included, when the recording shows where the
//! stack top is), where its callee's registers start, and how many values
//! each inlined function returns.

use super::*;

/// The frame of a call the trace holds without a real frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InlineCall {
    /// The arguments the call passes.
    pub(super) nargs: u32,
    /// A vararg callee's extra arguments, which `push_frame` leaves between
    /// the function and the callee's register 0.
    pub(super) n_varargs: u32,
    /// The values the caller wants (`C` - 1; -1 for all of them).
    pub(super) nresults: i32,
}

/// The values the `Return*` at `i` hands back, when the recording fixes
/// the count; `top` is the frame's stack top the recording left.
pub(super) fn return_count(inst: Inst, top: Option<u32>) -> Option<u32> {
    match inst.op() {
        Op::Return0 => Some(0),
        Op::Return1 => Some(1),
        Op::Return if inst.b() > 0 => Some(inst.b() - 1),
        Op::Return => top.and_then(|t| t.checked_sub(inst.a())),
        _ => None,
    }
}

/// One frame of the walk in [`inline_calls`].
struct Walk {
    /// The call that entered the frame (`None` at depth 0, or for a call
    /// the trace does not inline).
    call: Option<usize>,
    /// The stack top the last op that sets it left, in the frame's
    /// registers; `None` where the recording does not fix it.
    top: Option<u32>,
    /// The frame's extra arguments (a vararg function's).
    n_varargs: Option<u32>,
}

/// For each op of `record`: the frame of the call there, when the recording
/// followed it into a Lua function the trace can inline; and for each op,
/// the stack top of its frame before it runs, where the recording fixes it.
pub(super) fn inline_calls(record: &TraceRecord) -> (Vec<Option<InlineCall>>, Vec<Option<u32>>) {
    let n = record.ops.len();
    let mut calls: Vec<Option<InlineCall>> = vec![None; n];
    let mut tops: Vec<Option<u32>> = vec![None; n];
    let mut stack = vec![Walk {
        call: None,
        top: None,
        n_varargs: None,
    }];
    for (i, rop) in record.ops.iter().enumerate() {
        let d = rop.inline_depth as usize;
        // returns: the caller of a call that wanted every value has its
        // top just past the last one
        while stack.len() > d + 1 {
            let done = stack.pop().expect("deeper frame");
            let top = done.call.and_then(|c| {
                let call = &record.ops[c];
                let ret = record.ops[..i]
                    .iter()
                    .rposition(|r| r.inline_depth as usize == call.inline_depth as usize + 1)
                    .and_then(|r| return_count(record.ops[r].inst, tops[r]));
                (call.inst.c() == 0).then_some(())?;
                ret.map(|k| call.inst.a() + k)
            });
            if let Some(w) = stack.last_mut() {
                w.top = top;
            }
        }
        if stack.len() <= d {
            // a frame the walk did not see entered (a recording that does
            // not start in its head frame is refused elsewhere)
            break;
        }
        let ins = rop.inst;
        tops[i] = stack[d].top;
        match ins.op() {
            Op::Call => {
                let entered = record
                    .ops
                    .get(i + 1)
                    .is_some_and(|nx| nx.inline_depth as usize == d + 1);
                let shape = entered
                    .then(|| call_shape(record, i, stack[d].top))
                    .flatten();
                calls[i] = shape;
                stack[d].top = None;
                if entered {
                    stack.push(Walk {
                        call: shape.map(|_| i),
                        top: None,
                        n_varargs: shape.map(|s| s.n_varargs),
                    });
                }
            }
            Op::Vararg if ins.c() == 0 => {
                stack[d].top = (d > 0)
                    .then_some(stack[d].n_varargs)
                    .flatten()
                    .map(|m| ins.a() + m);
            }
            _ => {}
        }
    }
    // a call whose callee returns a count the recording does not fix
    // cannot hand back its values
    for i in 0..n {
        if calls[i].is_some() && !returns_fixed(record, i, &tops) {
            calls[i] = None;
        }
    }
    (calls, tops)
}

/// The frame of the `Call` at `i`, whose callee's first op follows it:
/// `None` when the trace cannot hold it. `top`: the caller's stack top.
fn call_shape(record: &TraceRecord, i: usize, top: Option<u32>) -> Option<InlineCall> {
    let rop = &record.ops[i];
    let callee = record.ops[i + 1].proto;
    let (a, b, c) = (rop.inst.a(), rop.inst.b(), rop.inst.c());
    let nargs = if b > 0 {
        b - 1
    } else {
        top?.checked_sub(a + 1)?
    };
    // 5.1's `arg` table is built by the call; 5.5's named vararg table and
    // its virtual indexing read the frame's extras from the stack
    if callee.has_compat_vararg_arg {
        return None;
    }
    let d = rop.inline_depth;
    let body_ok = record.ops[i + 1..]
        .iter()
        .take_while(|r| r.inline_depth > d)
        .filter(|r| r.inline_depth == d + 1)
        .all(|r| !matches!(r.inst.op(), Op::GetVarg | Op::VargIdx));
    if !body_ok {
        return None;
    }
    let n_varargs = if callee.is_vararg {
        nargs.saturating_sub(u32::from(callee.num_params))
    } else {
        0
    };
    // a self-link close steps into the next frame by `A + 1`
    if n_varargs > 0 && record.self_link_kind.is_some() {
        return None;
    }
    Some(InlineCall {
        nargs,
        n_varargs,
        nresults: c as i32 - 1,
    })
}

/// Whether the function the `Call` at `i` entered returns, within the
/// recording, a count the recording fixes (or does not return there).
fn returns_fixed(record: &TraceRecord, i: usize, tops: &[Option<u32>]) -> bool {
    let d = record.ops[i].inline_depth;
    record.ops[i + 1..]
        .iter()
        .enumerate()
        .take_while(|(_, r)| r.inline_depth > d)
        .filter(|(_, r)| {
            r.inline_depth == d + 1 && matches!(r.inst.op(), Op::Return | Op::Return0 | Op::Return1)
        })
        .all(|(k, r)| return_count(r.inst, tops[i + 1 + k]).is_some())
}

/// For each op, the register (counted from the head frame) holding the
/// closure its frame runs: the function slot of the call that entered the
/// frame, below the frame's extra arguments; 0 at depth 0.
pub(super) fn frame_funcs(record: &TraceRecord, op_offsets: &[u32]) -> Vec<u32> {
    let mut funcs: Vec<u32> = vec![0];
    record
        .ops
        .iter()
        .enumerate()
        .map(|(i, rop)| {
            let d = rop.inline_depth as usize;
            if d >= funcs.len() && i > 0 {
                let call = &record.ops[i - 1];
                funcs.push(op_offsets[i - 1] + call.inst.a());
            }
            funcs.truncate(d + 1);
            funcs.get(d).copied().unwrap_or(0)
        })
        .collect()
}

/// For each op, the registers (counted from the head frame: first, count)
/// it writes past what its instruction names: an inlined function's
/// return writes its caller's R[A] on, a vararg expansion R[A] on.
pub(super) fn inline_writes(
    record: &TraceRecord,
    op_offsets: &[u32],
    calls: &[Option<InlineCall>],
    tops: &[Option<u32>],
    funcs: &[u32],
) -> Vec<(u32, u32)> {
    // the call that entered each frame
    let mut entered: Vec<Option<usize>> = vec![None];
    record
        .ops
        .iter()
        .enumerate()
        .map(|(i, rop)| {
            let d = rop.inline_depth as usize;
            if d >= entered.len() && i > 0 {
                entered.push(Some(i - 1));
            }
            entered.truncate(d + 1);
            let call = entered[d].and_then(|c| calls[c]);
            let ins = rop.inst;
            match (ins.op(), call) {
                (Op::Return | Op::Return0 | Op::Return1, Some(c)) if d > 0 => {
                    let nret = return_count(ins, tops[i]).unwrap_or(0);
                    let wanted = u32::try_from(c.nresults).unwrap_or(nret);
                    (funcs[i], wanted)
                }
                (Op::Vararg, Some(c)) if d > 0 => {
                    let n = match ins.c() {
                        0 => c.n_varargs,
                        k => k - 1,
                    };
                    (op_offsets[i] + ins.a(), n)
                }
                _ => (0, 0),
            }
        })
        .collect()
}
