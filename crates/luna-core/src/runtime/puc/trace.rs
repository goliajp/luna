//! The accounting trace's lines about the PUC model (`gc-trace` only):
//! the state numbers of each dialect's `lgc.h`, the lines of a step, the
//! records' counts.

use super::step::StepHost;
use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::pace::St;

impl St {
    /// The state's number in the dialect's `lgc.h`.
    pub(crate) fn number(self, d: Dialect) -> u8 {
        use St::*;
        match d {
            Dialect::L51 => match self {
                Pause => 0,
                Propagate => 1,
                SweepString => 2,
                SweepAll => 3,
                _ => 4,
            },
            Dialect::L52 => match self {
                Propagate => 0,
                Atomic => 1,
                SweepString => 2,
                SweepUdata => 3,
                SweepAll => 4,
                _ => 5,
            },
            Dialect::L53 => match self {
                Propagate => 0,
                Atomic => 1,
                SweepAll => 2,
                SweepFinobj => 3,
                SweepTobefnz => 4,
                SweepEnd => 5,
                CallFin => 6,
                _ => 7,
            },
            _ => match self {
                Propagate => 0,
                EnterAtomic => 1,
                Atomic => 2,
                SweepAll => 3,
                SweepFinobj => 4,
                SweepTobefnz => 5,
                SweepEnd => 6,
                CallFin => 7,
                _ => 8,
            },
        }
    }
}

/// The accounting trace's line for a step, as PUC's `luaC_step` logs it
/// (`g` before, `G` after).
pub(crate) fn trace_step<H: StepHost + ?Sized>(h: &mut H, after: bool) {
    let d = h.heap().puc.dialect;
    let c = if after { 'G' } else { 'g' };
    let bytes = h.heap().bytes();
    let p = h.heap().puc.pace;
    let st = p.state().number(d);
    let kind = i32::from(h.heap().hooks.trace.generational);
    match d {
        Dialect::L51 => h.heap().hooks.trace.line(format_args!(
            "{c} {st} {bytes} {} {} {}",
            p.threshold(),
            p.estimate,
            p.dept
        )),
        _ => h.heap().hooks.trace.line(format_args!(
            "{c} {st} {bytes} {} {} {kind}",
            p.debt(d, bytes),
            p.estimate
        )),
    }
}

/// The end of a full collection in the trace: 5.5 with its `GCmarked`.
pub(crate) fn trace_full_end<H: StepHost + ?Sized>(h: &mut H) {
    if h.heap().puc.dialect == Dialect::L55 {
        let marked = h.heap().puc.pace.estimate;
        h.heap().hooks.trace.line(format_args!("x {marked}"));
    } else {
        h.heap().hooks.trace.line(format_args!("x"));
    }
}

impl Heap {
    /// gc-trace: the count moved by `delta` where the instrumented PUC's
    /// allocation function refused a growth.
    pub(crate) fn adjust_count(&mut self, delta: i64) {
        self.puc.count.bytes = (self.puc.count.bytes as i64 + delta) as usize;
    }
}
