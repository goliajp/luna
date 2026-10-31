//! `os.time`, `os.difftime` and `os.clock`, and the date-table fields they read and write.

use super::*;

pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .expect("the clock reads after 1970")
}

/// A time as the dialect returns it: a float before 5.3.
fn time_value(vm: &Vm, t: i64) -> Value {
    if vm.version() <= LuaVersion::Lua52 {
        Value::Float(t as f64)
    } else {
        Value::Int(t)
    }
}

/// `getfield` on the date table: the field's value as an `int`, `d` when
/// absent (`d < 0`: required). ≤5.2 take any number, truncated to `int`,
/// and treat a non-number as absent; 5.3+ want an integer and bound it.
fn getfield(vm: &mut Vm, t: Gc<Table>, key: &str, d: i32, delta: i64) -> Result<i32, LuaError> {
    let r = getfield_pushed(vm, t, key, d, delta);
    if r.is_ok() {
        vm.native_pop(1);
    }
    r
}

fn getfield_pushed(
    vm: &mut Vm,
    t: Gc<Table>,
    key: &str,
    d: i32,
    delta: i64,
) -> Result<i32, LuaError> {
    let v = vm.native_getfield(Value::Table(t), None, key.as_bytes())?;
    if vm.version() <= LuaVersion::Lua52 {
        return match argcheck::to_num(vm, v) {
            // (int)lua_tointeger: truncate, then wrap to 32 bits
            Some(n) => Ok((c_abi::tointeger52(vm.version(), n) as i32).wrapping_sub(delta as i32)),
            None if d < 0 => Err(field_error(vm, "field '%s' missing in date table", key)),
            None => Ok(d),
        };
    }
    let res = match argcheck::to_num(vm, v).and_then(num_exact) {
        Some(i) => i,
        None if !v.is_nil() => return Err(field_error(vm, "field '%s' is not an integer", key)),
        None if d < 0 => return Err(field_error(vm, "field '%s' missing in date table", key)),
        None => return Ok(d),
    };
    let in_range = if vm.version() == LuaVersion::Lua53 {
        let max = (i32::MAX / 2) as i64;
        (-max..=max).contains(&res)
    } else if res >= 0 {
        res - delta <= i32::MAX as i64
    } else {
        i32::MIN as i64 + delta <= res
    };
    if !in_range {
        return Err(field_error(vm, "field '%s' is out-of-bound", key));
    }
    Ok((res - delta) as i32)
}

/// `luaL_error(L, fmt, key)` for a field of the date table.
fn field_error(vm: &mut Vm, fmt: &str, key: &str) -> LuaError {
    let args = [crate::runtime::puc::fmt::FmtArg::S(key.as_bytes())];
    crate::vm::builtins::raise_lib(vm, fmt.as_bytes(), &args, 1)
}

/// `lua_tointegerx` from 5.3: only an integral value converts.
fn num_exact(n: crate::numeric::Num) -> Option<i64> {
    match n {
        crate::numeric::Num::Int(i) => Some(i),
        crate::numeric::Num::Float(f) => crate::runtime::value::f2i_exact(f),
    }
}

/// `setfield`: the value is pushed, then `lua_setfield` stores it.
fn setfield(vm: &mut Vm, t: Gc<Table>, key: &str, v: Value) -> Result<(), LuaError> {
    vm.native_push(1);
    vm.native_setfield(Value::Table(t), None, key.as_bytes(), v)
}

/// `setallfields`: write a broken-down time into `t` in the dialect's
/// order (the order is observable through `__newindex`).
pub(super) fn setallfields(vm: &mut Vm, t: Gc<Table>, tm: &Tm) -> Result<(), LuaError> {
    let civil = [
        tm.year,
        tm.month as i64,
        tm.day as i64,
        tm.hour as i64,
        tm.min as i64,
        tm.sec as i64,
    ];
    let days = (tm.yday as i64 + 1, tm.wday as i64 + 1);
    // UTC has no daylight saving time
    setfields(vm, t, civil, Some(days), Some(false))
}

/// Write year, month, day, hour, min, sec, then yday and wday when known
/// and isdst when known. ≤5.3 set them from seconds upwards, wday first.
fn setfields(
    vm: &mut Vm,
    t: Gc<Table>,
    civil: [i64; 6],
    days: Option<(i64, i64)>,
    isdst: Option<bool>,
) -> Result<(), LuaError> {
    const KEYS: [&str; 6] = ["year", "month", "day", "hour", "min", "sec"];
    let old = vm.version() <= LuaVersion::Lua53;
    let order: [usize; 6] = if old {
        [5, 4, 3, 2, 1, 0]
    } else {
        [0, 1, 2, 3, 4, 5]
    };
    for i in order {
        setfield(vm, t, KEYS[i], Value::Int(civil[i]))?;
    }
    if let Some((yday, wday)) = days {
        let pairs = if old {
            [("wday", wday), ("yday", yday)]
        } else {
            [("yday", yday), ("wday", wday)]
        };
        for (k, v) in pairs {
            setfield(vm, t, k, Value::Int(v))?;
        }
    }
    match isdst {
        Some(b) => setfield(vm, t, "isdst", Value::Bool(b)),
        None => Ok(()),
    }
}

pub(super) fn os_time(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if a.is_none_or_nil(vm, 0) {
        let v = time_value(vm, now());
        return vm.nat_ret1(fs, v);
    }
    let t = argcheck::check_table(vm, a, 0)?;
    vm.native_settop(1);
    let v = vm.version();
    let [year, mon, mday, hour, min, sec] = date_fields(vm, t)?;
    let isdst = vm.native_getfield(Value::Table(t), None, b"isdst")?;
    vm.native_pop(1);
    // a true isdst (`tm_isdst > 0`; nil is -1, false 0) in a zone without
    // daylight saving time: glibc's mktime takes DST to be one hour ahead
    // and moves the result an hour back
    let dst_shift = if isdst.truthy() { 3600 } else { 0 };
    let mut mon0 = mon as i64;
    let fields = (mday as i64, hour as i64, min as i64, sec as i64 - dst_shift);
    let secs = if crate::cerrno::Lib::HOST == crate::cerrno::Lib::Ucrt {
        let t = ucrt_mktime(year as i64, &mut mon0, fields);
        if t.is_none() {
            crate::cerrno::set(crate::cerrno::EINVAL);
        }
        t
    } else {
        mktime(
            year as i64 + 1900,
            mon0,
            fields.0,
            fields.1,
            fields.2,
            fields.3,
        )
    };
    // a time past `time_t` (32 bits on i386) is mktime's failure too
    let secs = secs.filter(|&s| c_abi::is_time(s));
    // 5.3+ write the fields back before checking the result, as PUC does:
    // normalised when the time exists, as given when it overflows (yday and
    // wday are then left alone: PUC writes whatever its `struct tm` held)
    if v >= LuaVersion::Lua53 {
        match secs {
            Some(s) => {
                let tm = gmtime(s).expect("mktime checked the year fits");
                setallfields(vm, t, &tm)?;
            }
            None => {
                let given = [
                    year as i64 + 1900,
                    mon0 + 1,
                    mday as i64,
                    hour as i64,
                    min as i64,
                    sec as i64,
                ];
                let dst = (!isdst.is_nil()).then(|| isdst.truthy());
                setfields(vm, t, given, None, dst)?;
            }
        }
    }
    // -1 is `mktime`'s failure value, so a time of exactly -1 fails too
    match secs.filter(|&s| s != -1) {
        Some(s) => {
            let r = time_value(vm, s);
            vm.nat_ret1(fs, r)
        }
        None if v <= LuaVersion::Lua52 => vm.nat_ret1(fs, Value::Nil),
        None => Err(raise_str(
            vm,
            "time result cannot be represented in this installation",
        )),
    }
}

/// The date table's fields year, month, day, hour, min and sec as
/// `os.time` reads them: up to 5.3 from seconds upwards, 5.4 from the year
/// down, which decides which bad field is reported.
fn date_fields(vm: &mut Vm, t: Gc<Table>) -> Result<[i32; 6], LuaError> {
    // (field, default, offset), in the order of the result
    let spec = [
        ("year", -1, 1900),
        ("month", -1, 1),
        ("day", -1, 0),
        ("hour", 12, 0),
        ("min", 0, 0),
        ("sec", 0, 0),
    ];
    let mut out = [0; 6];
    let order: [usize; 6] = if vm.version() >= LuaVersion::Lua54 {
        [0, 1, 2, 3, 4, 5]
    } else {
        [5, 4, 3, 2, 1, 0]
    };
    for i in order {
        let (key, d, delta) = spec[i];
        out[i] = getfield(vm, t, key, d, delta)?;
    }
    Ok(out)
}

/// The Universal CRT's `mktime` in a zone at UTC: `None` for a time
/// before 1970 or after 3000 (checked on the year first, before and after
/// the month is brought into range, which `mon0` keeps).
fn ucrt_mktime(
    tm_year: i64,
    mon0: &mut i64,
    (mday, hour, min, sec): (i64, i64, i64, i64),
) -> Option<i64> {
    const MAX_TIME: i64 = 0x7_9358_2AFF;
    let in_range = |y: i64| (69..=1102).contains(&y);
    let mut y = tm_year;
    if !in_range(y) {
        return None;
    }
    if !(0..=11).contains(mon0) {
        y += *mon0 / 12;
        *mon0 %= 12;
        if *mon0 < 0 {
            *mon0 += 12;
            y -= 1;
        }
        if !in_range(y) {
            return None;
        }
    }
    let t = mktime(y + 1900, *mon0, mday, hour, min, sec)?;
    (0..=MAX_TIME).contains(&t).then_some(t)
}
