use super::*;

/// PUC's `needclose`: some local is captured by a closure or is to be
/// closed, so returns and tail calls must close upvalues first.
pub(super) fn needs_close(p: &Proto) -> bool {
    crate::runtime::function_close::needs_close(&p.code, &p.protos)
}

/// `main`: the chunk's main function.
pub(super) fn vararg_byte(p: &Proto, d: Dialect, vatab: bool, main: bool) -> u8 {
    if !p.is_vararg {
        return 0;
    }
    match d {
        // a main function is VARARG_ISVARARG only: it has no `arg` local
        Dialect::V51 if main => 2,
        // VARARG_ISVARARG | VARARG_HASARG for the `arg` local, plus
        // VARARG_NEEDSARG when it holds the extra arguments
        Dialect::V51 if p.has_compat_vararg_arg => 7,
        Dialect::V51 => 3,
        // PF_VATAB or PF_VAHID
        Dialect::V55 if vatab => 2,
        _ => 1,
    }
}

/// 5.1 and 5.2 have one number type: luna's integers become floats.
pub(super) fn consts_for(d: Dialect, consts: Vec<Value>) -> Res<Vec<Value>> {
    if d > Dialect::V52 {
        return Ok(consts);
    }
    consts
        .into_iter()
        .map(|v| match v {
            Value::Int(i) if (i as f64) as i64 == i && i != i64::MAX => Ok(Value::Float(i as f64)),
            Value::Int(i) => Err(format!("{}: integer constant {i} has no float", d.name())),
            v => Ok(v),
        })
        .collect()
}

/// An instruction that skips the next one on some path skips exactly one
/// PUC instruction, so what follows it must still be a single instruction.
pub(super) fn check_skips(p: &Proto, pc_map: &[u32]) -> Res<()> {
    for (pc, i) in p.code.iter().enumerate() {
        let skips = matches!(
            i.op(),
            Op::LFalseSkip
                | Op::LTrueSkip
                | Op::Eq
                | Op::Lt
                | Op::Le
                | Op::EqK
                | Op::EqI
                | Op::LtI
                | Op::LeI
                | Op::GtI
                | Op::GeI
                | Op::Test
                | Op::TestSet
        );
        if skips && (pc + 2 >= pc_map.len() || pc_map[pc + 2] - pc_map[pc + 1] != 1) {
            return Err(format!(
                "instruction {} skips one that has no single-instruction form",
                pc + 1
            ));
        }
    }
    Ok(())
}
