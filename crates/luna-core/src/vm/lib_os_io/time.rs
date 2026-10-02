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
    let k = Value::Str(vm.heap.intern(key.as_bytes()));
    let v = vm.index_value(Value::Table(t), k)?;
    let missing = |vm: &mut Vm| raise_str(vm, &format!("field '{key}' missing in date table"));
    if vm.version() <= LuaVersion::Lua52 {
        return match argcheck::to_num(vm, v) {
            // (int)lua_tointeger: truncate, then wrap to 32 bits
            Some(n) => Ok((num_trunc(n) as i32).wrapping_sub(delta as i32)),
            None if d < 0 => Err(missing(vm)),
            None => Ok(d),
        };
    }
    let res = match argcheck::to_num(vm, v).and_then(num_exact) {
        Some(i) => i,
        None if !v.is_nil() => {
            return Err(raise_str(vm, &format!("field '{key}' is not an integer")));
        }
        None if d < 0 => return Err(missing(vm)),
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
        return Err(raise_str(vm, &format!("field '{key}' is out-of-bound")));
    }
    Ok((res - delta) as i32)
}

/// `lua_tointeger` before 5.3: a float truncated toward zero.
fn num_trunc(n: crate::numeric::Num) -> i64 {
    match n {
        crate::numeric::Num::Int(i) => i,
        crate::numeric::Num::Float(f) => f as i64,
    }
}

/// `lua_tointegerx` from 5.3: only an integral value converts.
fn num_exact(n: crate::numeric::Num) -> Option<i64> {
    match n {
        crate::numeric::Num::Int(i) => Some(i),
        crate::numeric::Num::Float(f) => crate::runtime::value::f2i_exact(f),
    }
}

fn setfield(vm: &mut Vm, t: Gc<Table>, key: &str, v: Value) -> Result<(), LuaError> {
    let k = Value::Str(vm.heap.intern(key.as_bytes()));
    vm.newindex_value(Value::Table(t), k, v)
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
        return Ok(vm.nat_return(fs, &[v]));
    }
    let t = argcheck::check_table(vm, a, 0)?;
    let v = vm.version();
    // ≤5.3 read the fields from seconds upwards, 5.4 from the year down; the
    // order decides which bad field is reported
    let (mut year, mut mon, mut mday, mut hour, mut min, mut sec) = (0, 0, 0, 0, 0, 0);
    if v >= LuaVersion::Lua54 {
        year = getfield(vm, t, "year", -1, 1900)?;
        mon = getfield(vm, t, "month", -1, 1)?;
        mday = getfield(vm, t, "day", -1, 0)?;
        hour = getfield(vm, t, "hour", 12, 0)?;
        min = getfield(vm, t, "min", 0, 0)?;
        sec = getfield(vm, t, "sec", 0, 0)?;
    } else {
        for (key, d, delta) in [
            ("sec", 0, 0),
            ("min", 0, 0),
            ("hour", 12, 0),
            ("day", -1, 0),
            ("month", -1, 1),
            ("year", -1, 1900),
        ] {
            let x = getfield(vm, t, key, d, delta)?;
            match key {
                "sec" => sec = x,
                "min" => min = x,
                "hour" => hour = x,
                "day" => mday = x,
                "month" => mon = x,
                _ => year = x,
            }
        }
    }
    let k = Value::Str(vm.heap.intern(b"isdst"));
    let isdst = vm.index_value(Value::Table(t), k)?;
    // a true isdst (`tm_isdst > 0`; nil is -1, false 0) in a zone without
    // daylight saving time: glibc's mktime takes DST to be one hour ahead
    // and moves the result an hour back
    let dst_shift = if isdst.truthy() { 3600 } else { 0 };
    let secs = mktime(
        year as i64 + 1900,
        mon as i64,
        mday as i64,
        hour as i64,
        min as i64,
        sec as i64 - dst_shift,
    );
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
                    mon as i64 + 1,
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
            Ok(vm.nat_return(fs, &[r]))
        }
        None if v <= LuaVersion::Lua52 => Ok(vm.nat_return(fs, &[Value::Nil])),
        None => Err(raise_str(
            vm,
            "time result cannot be represented in this installation",
        )),
    }
}

/// `l_checktime` (5.3+) or ≤5.2's `(time_t)luaL_checknumber`.
pub(super) fn check_time(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    if vm.version() <= LuaVersion::Lua52 {
        return Ok(argcheck::check_number(vm, a, i)? as i64);
    }
    argcheck::check_integer(vm, a, i)
}

pub(super) fn os_difftime(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t1 = check_time(vm, a, 0)?;
    // ≤5.2 default the second time to 0; 5.3 made it required
    let t2 = if vm.version() <= LuaVersion::Lua52 {
        argcheck::opt_number(vm, a, 1, 0.0)? as i64
    } else {
        check_time(vm, a, 1)?
    };
    Ok(vm.nat_return(fs, &[Value::Float(t1 as f64 - t2 as f64)]))
}

pub(super) fn os_clock(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    // C's clock() is processor time, which std cannot read; the time since
    // the Vm started stands in for it.
    let secs = vm.uptime().as_secs_f64();
    Ok(vm.nat_return(fs, &[Value::Float(secs)]))
}
