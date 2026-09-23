//! `collectgarbage` — the option set, argument handling, return shapes and
//! pacing-parameter storage of each PUC version.
//!
//! luna's collector is its own, so what `collectgarbage` must reproduce is the
//! *interface*: which options a dialect accepts, what each returns, and how a
//! parameter written reads back. PUC stores the parameters in version-specific
//! encodings that lose precision, and a program can observe that
//! (`setpause(150)` reads back 148 on 5.4, `param("pause", 123)` 118 on 5.5),
//! so the values are kept in the dialect's encoding.
//!
//! The parameters still drive luna's own pacing. A value is mapped onto luna's
//! knob in proportion to the dialect's default, so the default leaves luna's
//! pacing unchanged and doubling a parameter doubles its effect.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// Objects swept per unit of an explicit `collectgarbage("step", n)`.
const GC_STEP_OBJS: usize = 32;

/// luna's own pacing defaults: heap growth before a new cycle (%), sweep work
/// per safe point (see `Vm::maybe_collect_garbage`), and objects per step.
const LUNA_PAUSE: i64 = 200;
const LUNA_STEPMUL: i64 = 100;
const LUNA_STEPSIZE: i64 = 13;

/// A pacing parameter a dialect lets Lua read or write.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Param {
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
    fn default(&self, param: Param) -> Option<i32> {
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
            // LUAI_GCSTEPSIZE = 200 * sizeof(Table), 48 bytes on 64-bit
            (Lua55, StepSize) => Some(9600),
            (Lua55, MinorMul) => Some(20),
            (Lua55, MajorMinor) => Some(50),
            (Lua55, MinorMajor) => Some(70),
            _ => None,
        }
    }

    /// Store `v` the way the dialect does: a C `int` on ≤5.3 (5.3 raising a
    /// step multiplier below 40), a `lu_byte` holding `v / 4` or `v` on 5.4,
    /// a floating-point byte on 5.5.
    fn set(&mut self, param: Param, v: i32) {
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
    fn get(&self, param: Param) -> i64 {
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

    /// `param`'s current value on luna's scale for the matching knob.
    fn luna_scaled(&self, param: Param, luna_default: i64) -> i64 {
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

/// `collectgarbage([opt [, ...]])`.
pub(crate) fn nat_collectgarbage(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let opt = argcheck::check_option(vm, a, 0, Some("collect"), options(v))?;
    let opt = options(v)[opt];
    // ≤5.3 read the second argument as an int for every option.
    let ex = if v <= LuaVersion::Lua53 {
        argcheck::opt_integer(vm, a, 1, 0)? as i32
    } else {
        0
    };
    // 5.4+ `lua_gc` refuses to run inside a finalizer and the library reports
    // fail. ≤5.3 answer normally; luna's collector is not reentrant, so a
    // collection requested there does not run.
    if vm.gc_is_finalizing() && v >= LuaVersion::Lua54 {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    let out: Vec<Value> = match opt {
        "collect" => {
            if !vm.gc_is_finalizing() {
                if v <= LuaVersion::Lua53 {
                    // 5.1–5.3 propagate the first `__gc` error to the caller;
                    // 5.4 warns and continues (gc.lua 5.1 :255, 5.2 :346, 5.3 :360).
                    vm.collect_garbage_propagating()?;
                } else {
                    vm.collect_garbage();
                }
            }
            vec![Value::Int(0)]
        }
        "count" => {
            let bytes = vm.heap.bytes();
            let kb = Value::Float(bytes as f64 / 1024.0);
            if v == LuaVersion::Lua52 {
                // 5.2 alone also returns LUA_GCCOUNTB (gc.lua 5.2 :139 asserts
                // `k*1024 == floor(k)*1024 + b`).
                vec![kb, Value::Int((bytes % 1024) as i64)]
            } else {
                vec![kb]
            }
        }
        "step" => {
            let n = if v <= LuaVersion::Lua53 {
                i64::from(ex)
            } else {
                argcheck::opt_integer(vm, a, 1, 0)?
            };
            vec![Value::Bool(step(vm, n))]
        }
        "stop" => {
            vm.heap.gc_set_stopped(true);
            vec![Value::Int(0)]
        }
        "restart" => {
            vm.heap.gc_set_stopped(false);
            vec![Value::Int(0)]
        }
        // 5.2/5.3 stop the collector while a finalizer runs (GCTM clears
        // gcrunning), so a finalizer sees it as not running.
        "isrunning" => vec![Value::Bool(
            !vm.heap.gc_is_stopped() && !vm.gc_is_finalizing(),
        )],
        "setpause" => vec![Value::Int(set_param(vm, a, Param::Pause, ex)?)],
        "setstepmul" => vec![Value::Int(set_param(vm, a, Param::StepMul, ex)?)],
        "setmajorinc" => vec![Value::Int(set_param(vm, a, Param::MajorInc, ex)?)],
        "incremental" | "generational" => switch_mode(vm, a, opt)?,
        "param" => vec![param(vm, a)?],
        _ => unreachable!("check_option returned an index into options()"),
    };
    Ok(vm.nat_return(fs, &out))
}

fn options(v: LuaVersion) -> &'static [&'static str] {
    match v {
        LuaVersion::Lua51 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
        ],
        LuaVersion::Lua52 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
            "setmajorinc",
            "isrunning",
            "generational",
            "incremental",
        ],
        LuaVersion::Lua53 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
            "isrunning",
        ],
        LuaVersion::Lua54 | LuaVersion::MacroLua => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "setpause",
            "setstepmul",
            "isrunning",
            "generational",
            "incremental",
        ],
        LuaVersion::Lua55 => &[
            "stop",
            "restart",
            "collect",
            "count",
            "step",
            "isrunning",
            "generational",
            "incremental",
            "param",
        ],
    }
}

/// Advance the collector. A step of `n` sweeps a budgeted chunk and returns
/// true once a full cycle finishes; in generational mode a step is a full
/// minor collection.
///
/// `n` = 0 (also what an absent argument means) is PUC's basic step: a fixed
/// small one on ≤5.3 (`GCSTEPSIZE`; gc.lua's `dosteps(0) > 10` pins that it
/// takes many of them to finish a cycle), and one of the `stepsize`
/// parameter's size from 5.4 on, where stepsize 0 completes the cycle.
fn step(vm: &mut Vm, n: i64) -> bool {
    if vm.gc_mode_is_generational() {
        vm.collect_garbage();
        return false;
    }
    let budget = if n > 0 {
        (n as usize).saturating_mul(GC_STEP_OBJS)
    } else if vm.version() <= LuaVersion::Lua53 {
        GC_STEP_OBJS
    } else {
        match vm.gc_stepsize() {
            0 => usize::MAX,
            ss => (ss.max(1) as usize).saturating_mul(GC_STEP_OBJS),
        }
    };
    vm.gc_step(budget)
}

/// `setpause` / `setstepmul` / `setmajorinc`: store the new value (≤5.3 take
/// it from the already-read second argument, 5.4 reads it here; an absent
/// value is 0 and is stored too) and return the previous one.
fn set_param(vm: &mut Vm, a: Args, param: Param, ex: i32) -> Result<i64, LuaError> {
    let new = if vm.version() <= LuaVersion::Lua53 {
        ex
    } else {
        argcheck::opt_integer(vm, a, 1, 0)? as i32
    };
    let prev = vm.gc_params.get(param);
    vm.gc_params.set(param, new);
    sync_pacing(vm);
    Ok(prev)
}

/// `incremental` / `generational`. 5.2 switches and returns 0; 5.4 also sets
/// any nonzero parameters given; 5.4/5.5 return the previous mode's name.
fn switch_mode(vm: &mut Vm, a: Args, opt: &'static str) -> Result<Vec<Value>, LuaError> {
    let v = vm.version();
    if v == LuaVersion::Lua54 || v == LuaVersion::MacroLua {
        let params: &[Param] = if opt == "incremental" {
            &[Param::Pause, Param::StepMul, Param::StepSize]
        } else {
            &[Param::MinorMul, Param::MajorMul]
        };
        let mut values = Vec::with_capacity(params.len());
        for i in 0..params.len() as u32 {
            values.push(argcheck::opt_integer(vm, a, 1 + i, 0)? as i32);
        }
        for (&param, value) in params.iter().zip(values) {
            if value != 0 {
                vm.gc_params.set(param, value);
            }
        }
        sync_pacing(vm);
    }
    let prev = vm.gc_switch_mode(opt);
    Ok(vec![if v == LuaVersion::Lua52 {
        Value::Int(0)
    } else {
        Value::Str(vm.heap.intern(prev.as_bytes()))
    }])
}

/// 5.5 `param`: read a parameter, or set it (a negative value leaves it
/// unchanged) and return the previous value.
fn param(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    const NAMES: [&str; 6] = [
        "minormul",
        "majorminor",
        "minormajor",
        "pause",
        "stepmul",
        "stepsize",
    ];
    const MAP: [Param; 6] = [
        Param::MinorMul,
        Param::MajorMinor,
        Param::MinorMajor,
        Param::Pause,
        Param::StepMul,
        Param::StepSize,
    ];
    let which = MAP[argcheck::check_option(vm, a, 1, None, &NAMES)?];
    let value = argcheck::opt_integer(vm, a, 2, -1)? as i32;
    let prev = vm.gc_params.get(which);
    if value >= 0 {
        vm.gc_params.set(which, value);
        sync_pacing(vm);
    }
    Ok(Value::Int(prev))
}

/// Carry the Lua-visible parameters over to luna's collector knobs.
fn sync_pacing(vm: &mut Vm) {
    let p = &vm.gc_params;
    let pause = p.luna_scaled(Param::Pause, LUNA_PAUSE);
    let stepmul = p.luna_scaled(Param::StepMul, LUNA_STEPMUL);
    let stepsize = match p.default(Param::StepSize) {
        // stepsize 0 means "a step completes the cycle" and must stay 0
        Some(_) if p.get(Param::StepSize) == 0 => 0,
        Some(_) => p.luna_scaled(Param::StepSize, LUNA_STEPSIZE).max(1),
        None => LUNA_STEPSIZE,
    };
    vm.set_gc_pacing(pause, stepmul, stepsize);
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(got, [20, 50, 68, 250, 200, 9600]);
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
