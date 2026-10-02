use super::*;

/// Verify that `is_mvp_trace_op` accepts the expected op set and rejects
/// others. Pins the whitelist without requiring a live LLVM context.
#[test]
fn mvp_whitelist_coverage() {
    for op in [
        Op::LoadI,
        Op::Move,
        Op::Add,
        Op::Sub,
        Op::Mul,
        Op::Mod,
        Op::Lt,
        Op::Le,
        Op::Eq,
        Op::Jmp,
    ] {
        assert!(is_mvp_trace_op(op), "{op:?} should be in MVP whitelist");
    }
    for op in [
        Op::Call,
        Op::TailCall,
        Op::GetUpval,
        Op::GetTabUp,
        Op::GetField,
        Op::Return0,
        Op::Return1,
        Op::LoadNil,
    ] {
        assert!(
            !is_mvp_trace_op(op),
            "{op:?} should NOT be in MVP whitelist"
        );
    }
}
