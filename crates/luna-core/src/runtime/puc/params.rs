//! The pacing parameters of `collectgarbage`, kept in each dialect's
//! storage encoding.

use crate::version::LuaVersion;

/// PUC 5.5's `sizeof(Table)` on this target.
const TABLE_55: usize =
    crate::runtime::puc::size::PucSizes::of(crate::runtime::dialect::Dialect::L55).table;

/// A pacing parameter a dialect lets Lua read or write.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Param {
    Pause,
    StepMul,
    StepSize,
    /// 5.2 `setmajorinc`.
    MajorInc,
    /// 5.4 `generational` first argument; 5.5 `minormul`.
    MinorMul,
    /// 5.4 `generational` second argument.
    MajorMul,
    /// 5.5 `majorminor`.
    MajorMinor,
    /// 5.5 `minormajor`.
    MinorMajor,
}

const PARAMS: [Param; 8] = [
    Param::Pause,
    Param::StepMul,
    Param::StepSize,
    Param::MajorInc,
    Param::MinorMul,
    Param::MajorMul,
    Param::MajorMinor,
    Param::MinorMajor,
];

/// The parameters in the dialect's storage encoding.
pub(crate) struct GcParams {
    version: LuaVersion,
    stored: [i64; PARAMS.len()],
}

impl GcParams {
    pub(crate) fn new(version: LuaVersion) -> Self {
        let mut p = GcParams {
            version,
            stored: [0; PARAMS.len()],
        };
        for param in PARAMS {
            if let Some(v) = p.default(param) {
                p.set(param, v);
            }
        }
        p
    }

    fn slot(param: Param) -> usize {
        PARAMS
            .iter()
            .position(|&q| q == param)
            .expect("every Param is listed in PARAMS")
    }

    /// PUC's compile-time default, or `None` where the dialect has no such
    /// parameter.
    pub(crate) fn default(&self, param: Param) -> Option<i32> {
        use LuaVersion::*;
        use Param::*;
        match (self.version, param) {
            (Lua51 | Lua52 | Lua53, Pause | StepMul) => Some(200),
            (Lua52, MajorInc) => Some(200),
            (Lua54 | MacroLua, Pause) => Some(200),
            (Lua54 | MacroLua, StepMul) => Some(100),
            (Lua54 | MacroLua, StepSize) => Some(13),
            (Lua54 | MacroLua, MinorMul) => Some(20),
            (Lua54 | MacroLua, MajorMul) => Some(100),
            (Lua55, Pause) => Some(250),
            (Lua55, StepMul) => Some(200),
            // LUAI_GCSTEPSIZE = 200 * sizeof(Table)
            (Lua55, StepSize) => Some(200 * TABLE_55 as i32),
            (Lua55, MinorMul) => Some(20),
            (Lua55, MajorMinor) => Some(50),
            (Lua55, MinorMajor) => Some(70),
            _ => None,
        }
    }

    /// Store `v` the way the dialect does: a C `int` on ≤5.3 (5.3 raising a
    /// step multiplier below 40), a `lu_byte` holding `v / 4` or `v` on 5.4,
    /// a floating-point byte on 5.5.
    pub(crate) fn set(&mut self, param: Param, v: i32) {
        use LuaVersion::*;
        let stored = match self.version {
            Lua51 | Lua52 => i64::from(v),
            Lua53 if param == Param::StepMul => i64::from(v.max(40)),
            Lua53 => i64::from(v),
            Lua54 | MacroLua => match param {
                Param::Pause | Param::StepMul | Param::MajorMul => i64::from((v / 4) as u8),
                _ => i64::from(v as u8),
            },
            Lua55 => i64::from(code_param(v as u32)),
        };
        self.stored[Self::slot(param)] = stored;
    }

    /// The value Lua reads back.
    pub(crate) fn get(&self, param: Param) -> i64 {
        let s = self.stored[Self::slot(param)];
        match self.version {
            LuaVersion::Lua54 | LuaVersion::MacroLua => match param {
                Param::Pause | Param::StepMul | Param::MajorMul => s * 4,
                _ => s,
            },
            LuaVersion::Lua55 => apply_param(s as u8, 100),
            _ => s,
        }
    }

    /// `param` applied to `x` as the collector does (5.5
    /// `applygcparam`: the floating-point byte times `x`; elsewhere the
    /// percentage of `x`).
    pub(crate) fn apply(&self, param: Param, x: i64) -> i64 {
        match self.version {
            LuaVersion::Lua55 => apply_param(self.stored[Self::slot(param)] as u8, x),
            _ => x.saturating_mul(self.get(param)) / 100,
        }
    }

    /// `param`'s current value on luna's scale for the matching knob.
    pub(crate) fn luna_scaled(&self, param: Param, luna_default: i64) -> i64 {
        match self.default(param) {
            Some(d) => self.get(param) * luna_default / i64::from(d),
            None => luna_default,
        }
    }
}

/// `luaO_codeparam` (5.5): a percentage as a floating-point byte, 4-bit
/// mantissa and 4-bit exponent, rounding up.
fn code_param(p: u32) -> u8 {
    if u64::from(p) >= (0x1F_u64 << (0xF - 7 - 1)) * 100 {
        return 0xFF;
    }
    let p = (p * 128).div_ceil(100);
    if p < 0x10 {
        return p as u8;
    }
    let log = ceil_log2(p + 1) - 5;
    (((p >> log) - 0x10) | ((log + 1) << 4)) as u8
}

/// `luaO_ceillog2`.
fn ceil_log2(x: u32) -> u32 {
    32 - (x - 1).leading_zeros()
}

/// `luaO_applyparam` (5.5): `p` (a floating-point byte) times `x`.
fn apply_param(p: u8, x: i64) -> i64 {
    let mut m = i64::from(p & 0xF);
    let mut e = i32::from(p >> 4);
    if e > 0 {
        e -= 1;
        m += 0x10;
    }
    e -= 7;
    if e >= 0 { (x * m) << e } else { (x * m) >> -e }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::lib_gc::{LUNA_PAUSE, LUNA_STEPMUL};

    // Values measured on stock PUC 5.5.1: collectgarbage("param", "pause", v)
    // followed by a read.
    #[test]
    fn param_encoding_reads_back_like_puc_5_5() {
        for (v, back) in [
            (1, 1),
            (7, 7),
            (15, 15),
            (16, 16),
            (99, 96),
            (100, 100),
            (101, 100),
            (123, 118),
            (128, 125),
            (1000, 1000),
            (12345, 12000),
            (1_000_000, 396_800),
        ] {
            assert_eq!(apply_param(code_param(v), 100), back, "param {v}");
        }
    }

    #[test]
    fn defaults_read_back_like_puc_5_5() {
        let p = GcParams::new(LuaVersion::Lua55);
        let got: Vec<i64> = [
            Param::MinorMul,
            Param::MajorMinor,
            Param::MinorMajor,
            Param::Pause,
            Param::StepMul,
            Param::StepSize,
        ]
        .iter()
        .map(|&q| p.get(q))
        .collect();
        assert_eq!(got, [20, 50, 68, 250, 200, 200 * TABLE_55 as i64]);
    }

    #[test]
    fn five_four_stores_a_quarter_in_a_byte() {
        let mut p = GcParams::new(LuaVersion::Lua54);
        p.set(Param::Pause, 150);
        assert_eq!(p.get(Param::Pause), 148);
        p.set(Param::Pause, 2000); // 500 does not fit a lu_byte: 244
        assert_eq!(p.get(Param::Pause), 976);
    }

    #[test]
    fn defaults_leave_luna_pacing_unchanged() {
        for v in [
            LuaVersion::Lua51,
            LuaVersion::Lua52,
            LuaVersion::Lua53,
            LuaVersion::Lua54,
            LuaVersion::Lua55,
        ] {
            let p = GcParams::new(v);
            assert_eq!(p.luna_scaled(Param::Pause, LUNA_PAUSE), LUNA_PAUSE, "{v:?}");
            assert_eq!(
                p.luna_scaled(Param::StepMul, LUNA_STEPMUL),
                LUNA_STEPMUL,
                "{v:?}"
            );
        }
    }
}
