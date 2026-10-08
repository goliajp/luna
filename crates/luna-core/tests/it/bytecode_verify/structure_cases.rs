//! Opcodes, register ranges, jump targets, paired instructions and nesting limits.

use super::*;

#[test]
fn invalid_opcode() {
    refused(
        ADD,
        |p| p.code[0] = (p.code[0] & !0x7F) | 0x7F,
        "invalid opcode 127",
    );
}

#[test]
fn register_beyond_max_stack() {
    refused(
        ADD,
        |p| {
            let pc = find(p, Op::Add);
            set(p, pc, |i| with_a(i, 200));
        },
        "register 200 out of range",
    );
}

#[test]
fn register_run_beyond_max_stack() {
    refused(
        "return function() local a, b, c return a end",
        |p| {
            let pc = find(p, Op::LoadNil);
            set(p, pc, |i| with_b(i, 250));
        },
        "out of range (stack size",
    );
}

#[test]
fn call_results_beyond_max_stack() {
    refused(
        "return function() local f = print f() end",
        |p| {
            let pc = find(p, Op::Call);
            set(p, pc, |i| with_c(i, 100));
        },
        "out of range (stack size",
    );
}

#[test]
fn jump_outside_the_code() {
    refused(
        "return function(x) if x then x = 1 end return x end",
        |p| {
            let pc = find(p, Op::Jmp);
            p.code[pc] = Inst::isj(Op::Jmp, 1000).0;
        },
        "jumps outside the code",
    );
}

#[test]
fn falls_off_the_end() {
    refused(
        ADD,
        |p| {
            let last = p.code.len() - 1;
            p.code[last] = Inst::iabc(Op::Move, 0, 0, 0, false).0;
        },
        "falls off the end of the code",
    );
}

#[test]
fn empty_code() {
    refused(
        ADD,
        |p| {
            p.code.clear();
            p.lines.clear();
        },
        "no instructions",
    );
}

#[test]
fn loadkx_without_extra_arg() {
    refused(
        "return function() return 'k' end",
        |p| {
            let pc = find(p, Op::LoadK);
            let a = Inst(p.code[pc]).a();
            p.code[pc] = Inst::iabx(Op::LoadKx, a, 0).0;
        },
        "not followed by its extra argument",
    );
}

#[test]
fn extra_arg_executed() {
    refused(
        ADD,
        |p| p.code[0] = Inst::iax(Op::ExtraArg, 0).0,
        "executed as an instruction",
    );
}

#[test]
fn for_prep_without_its_loop() {
    refused(
        "return function() local s = 0 for i = 1, 3 do s = s + i end return s end",
        |p| {
            let pc = find(p, Op::ForPrep55);
            set(p, pc, |i| with_bx(i, i.bx() + 1));
        },
        "no matching ForLoop",
    );
}

#[test]
fn for_loop_without_its_prep() {
    refused(
        "return function() local s = 0 for i = 1, 3 do s = s + i end return s end",
        |p| {
            let pc = find(p, Op::ForPrep55);
            p.code[pc] = Inst::isj(Op::Jmp, 0).0;
        },
        "not paired with a ForPrep",
    );
}

#[test]
fn tforcall_not_followed_by_tforloop() {
    refused(
        "return function(t) for k in pairs(t) do end end",
        |p| {
            let pc = find(p, Op::TForLoop55);
            p.code[pc] = Inst::iabc(Op::Return0, 0, 0, 0, false).0;
        },
        "TForLoop",
    );
}

#[test]
fn tforloop_without_its_tforcall() {
    refused(
        "return function(t) for k in pairs(t) do end end",
        |p| {
            // a second TForLoop, looping on itself, before the final return:
            // no TForCall precedes it
            let a = Inst(p.code[find(p, Op::TForCall55)]).a();
            let at = p.code.len() - 1;
            p.code.insert(at, Inst::iabx(Op::TForLoop55, a, 1).0);
            p.lines.insert(at, 1);
        },
        "not preceded by its TForCall",
    );
}

#[test]
fn compare_not_followed_by_jmp() {
    refused(
        "return function(x, y) if x < y then return 1 end return 2 end",
        |p| {
            let pc = find(p, Op::Lt);
            p.code[pc + 1] = Inst::iabc(Op::Move, 0, 0, 0, false).0;
        },
        "not followed by a Jmp",
    );
}

#[test]
fn open_call_without_producer() {
    refused(
        "return function(...) return print(...) end",
        |p| {
            let pc = find(p, Op::Vararg);
            // results fixed at one value: the B = 0 consumer has no top
            set(p, pc, |i| with_c(i, 2));
        },
        "stack top no instruction set",
    );
}

#[test]
fn line_info_length_mismatch() {
    refused(ADD, |p| p.lines.push(1), "line entries for");
}

#[test]
fn params_exceed_max_stack() {
    refused(
        ADD,
        |p| p.num_params = p.max_stack + 1,
        "parameters exceed stack size",
    );
}

#[test]
fn nested_upvalue_descriptor_out_of_range() {
    refused(
        "return function() local u = 1 return function() return u end end",
        |p| p.protos[0].upvals[0].1 = 250,
        "captures register 250 out of range",
    );
}

#[test]
fn nested_function_is_checked() {
    refused(
        "return function() return function(x, y) return x + y end end",
        |p| {
            let pc = find(&p.protos[0], Op::Add);
            set(&mut p.protos[0], pc, |i| with_b(i, 222));
        },
        "function at line",
    );
}

#[test]
fn functions_nested_too_deep() {
    refused(
        "return function() local function g() end end",
        |p| {
            let mut chain = p.protos[0].clone();
            for _ in 0..300 {
                let mut outer = p.clone();
                outer.protos = vec![chain];
                chain = outer;
            }
            *p = chain;
        },
        "functions nested too deep",
    );
}
