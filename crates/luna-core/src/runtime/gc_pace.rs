//! The pace of PUC's collector, by dialect: when the next step is due, how
//! much work a step does, and how long a cycle's pause is. Arithmetic on
//! PUC's quantities only; the constants are each version's (`lgc.c`,
//! `lgc.h`, `luaconf.h`), the sizes the target's ([`PucSizes`]).

use crate::runtime::puc_size::PucSizes;
use crate::runtime::table::Dialect;

/// PUC's collector states. Their numbers in each version's `lgc.h` are
/// [`St::number`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum St {
    Pause,
    Propagate,
    /// 5.4 and 5.5: the step that runs `atomic`
    EnterAtomic,
    /// 5.2 and 5.3: the step that runs `atomic`
    Atomic,
    /// 5.1 and 5.2: the string table, a bucket (5.1) or `GCSWEEPMAX`
    /// buckets (5.2) a step
    SweepString,
    /// 5.2: the objects with finalizers
    SweepUdata,
    /// 5.1 and 5.2 `GCSsweep`, 5.3 on `GCSswpallgc`
    SweepAll,
    SweepFinobj,
    SweepTobefnz,
    SweepEnd,
    /// 5.3 on: the finalizers
    CallFin,
    /// 5.1: the finalizers
    Finalize,
}

impl St {
    /// The state's number in the dialect's `lgc.h`.
    #[cfg(feature = "gc-trace")]
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

/// The collector's pace: PUC's own fields, kept against luna's count
/// (`Heap::bytes`, PUC's `totalbytes` as `gettotalbytes` reads it).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pace {
    pub(crate) state: St,
    /// where the next step falls due: 5.1 `GCthreshold`; 5.2 to 5.4 the
    /// `totalbytes` field (the count less `GCdebt`); 5.5 `GCtotalbytes`
    /// (the count plus `GCdebt`)
    base: i64,
    /// 5.1 `estimate`, 5.2 to 5.4 `GCestimate`, 5.5 `GCmarked`
    pub(crate) estimate: i64,
    /// 5.1 `gcdept`
    pub(crate) dept: i64,
    /// 5.2 and 5.3 `sweepstrgc` (5.1 too): the string table's buckets swept
    pub(crate) strt_swept: i64,
    /// what is left of a sweep over one of PUC's lists luna keeps on its
    /// one list (the objects with finalizers, those to be finalized)
    pub(crate) fin_left: Option<i64>,
    /// 5.5: markings count what they mark (`GCmarked`)
    pub(crate) count_marked: bool,
    /// the threads the VM holds, as the step under way found them
    pub(crate) run: Option<super::heap::Running>,
}

/// 5.1 `GCSTEPSIZE`
pub(crate) const STEPSIZE_51: i64 = 1024;
/// 5.1 `GCSWEEPMAX`, `GCSWEEPCOST`
pub(crate) const SWEEPMAX_51: i64 = 40;
pub(crate) const SWEEPCOST_51: i64 = 10;
/// 5.2 and 5.3 `STEPMULADJ`; 5.2 to 5.4 `PAUSEADJ`
const STEPMULADJ: i64 = 200;
const PAUSEADJ: i64 = 100;
/// 5.4 `GCSWEEPMAX`
pub(crate) const SWEEPMAX_54: i64 = 100;
/// 5.5 `GCSWEEPMAX`
pub(crate) const SWEEPMAX_55: i64 = 20;

impl Pace {
    pub(crate) fn new() -> Pace {
        Pace {
            state: St::Pause,
            base: 0,
            estimate: 0,
            dept: 0,
            strt_swept: 0,
            fin_left: None,
            count_marked: false,
            run: None,
        }
    }

    /// 5.2 to 5.5 `GCdebt` with the count at `bytes`.
    pub(crate) fn debt(&self, d: Dialect, bytes: usize) -> i64 {
        match d {
            Dialect::L55 => self.base - bytes as i64,
            _ => bytes as i64 - self.base,
        }
    }

    /// `luaE_setdebt(g, debt)` with the count at `bytes`.
    pub(crate) fn set_debt(&mut self, d: Dialect, bytes: usize, debt: i64) {
        self.base = match d {
            Dialect::L55 => bytes as i64 + debt,
            _ => bytes as i64 - debt,
        };
    }

    /// 5.1 `GCthreshold`.
    pub(crate) fn threshold(&self) -> i64 {
        self.base
    }

    pub(crate) fn set_threshold(&mut self, t: i64) {
        self.base = t;
    }

    /// The count at which a step falls due: 5.1 `totalbytes >=
    /// GCthreshold`, 5.2 to 5.4 `GCdebt > 0`, 5.5 `GCdebt <= 0`.
    pub(crate) fn limit(&self, d: Dialect) -> usize {
        let l = match d {
            Dialect::L51 | Dialect::L55 => self.base,
            _ => self.base.saturating_add(1),
        };
        l.max(0) as usize
    }
}

/// The work a 5.1 `luaC_step` may do (`lim`), `stepmul` the parameter.
pub(crate) fn step_limit_51(stepmul: i64) -> i64 {
    match (STEPSIZE_51 / 100) * stepmul {
        0 => i64::MAX / 4,
        l => l,
    }
}

/// 5.1 `setthreshold`.
pub(crate) fn threshold_51(estimate: i64, pause: i64) -> i64 {
    (estimate / 100) * pause
}

/// 5.2 and 5.3 `GCSTEPSIZE` (`100 * sizeof(TString)`).
pub(crate) fn stepsize_52(s: &PucSizes) -> i64 {
    100 * s.str_hdr as i64
}

/// 5.2 and 5.3 `GCSWEEPCOST` and `GCSWEEPMAX`.
pub(crate) fn sweepcost_52(s: &PucSizes) -> i64 {
    (s.str_hdr as i64 + 4) / 4
}

pub(crate) fn sweepmax_52(s: &PucSizes) -> i64 {
    (stepsize_52(s) / sweepcost_52(s)) / 4
}

/// The work a 5.2 `incstep` (5.3 `getdebt`) starts with: the debt in
/// work units, `stepmul` the parameter (5.3 stores at least 40).
pub(crate) fn work_52(d: Dialect, debt: i64, stepmul: i64) -> i64 {
    if d == Dialect::L53 && debt <= 0 {
        return 0;
    }
    (debt / STEPMULADJ + 1) * stepmul
}

/// The debt a 5.2 / 5.3 step leaves when the cycle goes on: the work
/// still owed (`debt`, negative when done ahead) back in bytes.
pub(crate) fn debt_after_52(debt: i64, stepmul: i64) -> i64 {
    (debt / stepmul) * STEPMULADJ
}

/// 5.2 to 5.4 `setpause`: the debt a cycle's end leaves, the count at
/// `bytes`, `estimate` the estimate and `pause` the parameter (5.4: its
/// stored value times 4).
pub(crate) fn pause_debt(d: Dialect, bytes: usize, estimate: i64, pause: i64) -> i64 {
    let est = estimate / PAUSEADJ;
    let threshold = if est > 0 && pause >= i64::MAX / est {
        i64::MAX
    } else {
        est * pause
    };
    let debt = bytes as i64 - threshold;
    match d {
        // 5.4 never sets a positive debt; 5.2 and 5.3 set the threshold
        Dialect::L54 => debt.min(0),
        _ => debt,
    }
}

/// 5.4 `incstep`'s debt and step size in work units (`WORK2MEM` is
/// `sizeof(TValue)`), `stepmul` the stored parameter times 4 (or 1),
/// `stepsize` the log2 parameter.
pub(crate) fn work_54(s: &PucSizes, debt: i64, stepmul: i64, stepsize: i64) -> (i64, i64) {
    let w2m = s.tvalue as i64;
    let size = if stepsize <= 62 {
        ((1i64 << stepsize) / w2m) * stepmul
    } else {
        i64::MAX
    };
    ((debt / w2m) * stepmul, size)
}

/// The debt a 5.4 step leaves when the cycle goes on.
pub(crate) fn debt_after_54(s: &PucSizes, debt: i64, stepmul: i64) -> i64 {
    (debt / stepmul) * s.tvalue as i64
}

/// 5.5 `setpause`: the debt a cycle's end leaves (never negative), the
/// threshold being `pause` (applied) times `marked`.
pub(crate) fn pause_debt_55(bytes: usize, threshold: i64) -> i64 {
    (threshold - bytes as i64).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debt_and_limit_by_dialect() {
        let mut p = Pace::new();
        p.set_debt(Dialect::L54, 1000, -200);
        assert_eq!(p.debt(Dialect::L54, 1000), -200);
        // due once the debt is positive
        assert_eq!(p.limit(Dialect::L54), 1201);
        let mut p = Pace::new();
        p.set_debt(Dialect::L55, 1000, 300);
        assert_eq!(p.debt(Dialect::L55, 1100), 200);
        // due once the debt reaches 0
        assert_eq!(p.limit(Dialect::L55), 1300);
    }

    #[test]
    fn five_one_step_and_threshold() {
        assert_eq!(step_limit_51(200), 2000);
        assert!(step_limit_51(0) > 1 << 40);
        assert_eq!(threshold_51(1050, 200), 2000);
    }

    #[test]
    fn five_two_work_and_pause() {
        assert_eq!(work_52(Dialect::L52, 0, 200), 200);
        assert_eq!(work_52(Dialect::L53, 0, 200), 0);
        assert_eq!(work_52(Dialect::L53, 401, 200), 600);
        assert_eq!(debt_after_52(-2400, 200), -2400);
        assert_eq!(pause_debt(Dialect::L53, 1000, 1000, 200), -1000);
        assert_eq!(pause_debt(Dialect::L54, 3000, 1000, 200), 0);
    }
}
