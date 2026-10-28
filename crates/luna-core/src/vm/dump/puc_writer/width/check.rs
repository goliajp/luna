//! The encoders' output checked against [`Layout`], in debug and gc-trace
//! builds.

use super::*;

/// What the encoder emitted for `p` against [`Layout::width`], when the
/// widths do not need the constant indices.
pub(in crate::vm::dump::puc_writer) fn check(
    p: &Proto,
    d: Dialect,
    body: &super::super::asm::Body,
) {
    if p.consts.len() > 255 || !body.k_added.is_empty() {
        return;
    }
    let lay = Layout {
        modern: d >= Dialect::V54,
        v51: d == Dialect::V51,
        v55: d == Dialect::V55,
        vatab: vatab(p, d),
        encoded: None,
    };
    let mut lines = Vec::new();
    lay.prologue(p, |l| lines.push(l));
    assert_eq!(
        body.pc_map[0] as usize,
        lines.len(),
        "{}: prologue",
        d.name()
    );
    for pc in 0..p.code.len() {
        let n = body.pc_map[pc + 1] - body.pc_map[pc];
        assert_eq!(lay.width(p, pc), n, "{}: width at {pc}", d.name());
        lay.code(p, pc, 0, |l| lines.push(l));
    }
    if !p.lines.is_empty() {
        assert_eq!(body.lines, lines, "{}: lines", d.name());
    }
}

/// A text load's functions as the encoder lays them out against
/// [`Layout`]: the encoder adds no constant to code luna compiled.
pub(crate) fn check_compiled(proto: &Proto, version: LuaVersion) {
    let Some(d) = Dialect::of(version) else {
        return;
    };
    if let Ok(sh) = super::super::shape_one(proto, d) {
        assert!(
            sh.k_added.is_empty(),
            "the encoder added a constant to compiled code"
        );
    }
    for c in proto.protos.iter() {
        check_compiled(c, version);
    }
}
