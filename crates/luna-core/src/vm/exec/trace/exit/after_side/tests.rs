//! The write-back by a side exit's plan writes the frame as the one register
//! by register does, after a side trace ran: every kind of exit tag, in the
//! frame and past it, with entry tags checked and unchecked.

use crate::jit::trace::{ENTRY_TAG_ANY, ExitTag, TagResKind};
use crate::jit::trace_types::installed::exit_plan::PlannedExit;
use crate::runtime::Value;
use crate::runtime::value::raw;
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

const KINDS: [ExitTag; 9] = [
    ExitTag::Untouched,
    ExitTag::Int,
    ExitTag::Float,
    ExitTag::Nil,
    ExitTag::Bool,
    ExitTag::Untouched,
    ExitTag::Int,
    ExitTag::Untouched,
    ExitTag::Float,
];

/// Entry tags a parent leaves for the head frame: checked ones (an integer,
/// a float, nil, false) and unchecked ones.
const ENTRY: [u8; 4] = [raw::INT, ENTRY_TAG_ANY, raw::FLOAT, raw::NIL];

/// The frame both write-backs start from.
fn frame(len: usize) -> Vec<Value> {
    (0..len).map(|i| Value::Int(1000 + i as i64)).collect()
}

/// What each write-back leaves on a frame of `len` registers, `max_stack`
/// of them in the head frame, after an exit with tags `tags`.
fn both(tags: &[ExitTag], max_stack: usize) -> (Vec<Value>, Vec<Value>) {
    let len = tags.len();
    let regs: Vec<i64> = (0..len as i64).map(|i| i * 7 + 1).collect();
    let entry: Vec<u8> = (0..max_stack).map(|i| ENTRY[i % ENTRY.len()]).collect();
    let mut out = Vec::new();
    for planned in [true, false] {
        let mut vm = Vm::new(LuaVersion::Lua54);
        let base = vm.stack.len();
        for v in frame(len) {
            vm.stack.push(v).unwrap();
        }
        if planned {
            let e = PlannedExit::of(tags);
            vm.trace_restore_after_side(base, max_stack, &e, &regs, &entry);
        } else {
            vm.trace_restore_slots(
                base,
                max_stack,
                0..0,
                false,
                TagResKind::Mixed,
                tags,
                &regs,
                &entry,
                true,
            );
        }
        out.push(vm.stack[base..base + len].to_vec());
    }
    let b = out.pop().unwrap();
    (out.pop().unwrap(), b)
}

#[test]
fn the_plan_writes_back_as_register_by_register() {
    for len in 1..=KINDS.len() {
        for shift in 0..KINDS.len() {
            let tags: Vec<ExitTag> = (0..len).map(|i| KINDS[(i + shift) % KINDS.len()]).collect();
            for max_stack in 0..=len {
                let (plan, slots) = both(&tags, max_stack);
                assert_eq!(
                    format!("{plan:?}"),
                    format!("{slots:?}"),
                    "tags {tags:?}, frame of {max_stack}"
                );
            }
        }
    }
}
