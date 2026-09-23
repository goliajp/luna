//! The os library, plus the entry point that opens io and os together and
//! the base functions that read files (`loadfile`, `dofile`). Shaped per
//! dialect after loslib.c 5.1–5.5.
//!
//! Time is UTC throughout: luna-core links no libc, so it has no time zone
//! database. `os.date` without `!` and `os.time` read and write UTC broken-
//! down time, which keeps `os.time(os.date("*t", t)) == t` exact.

use crate::runtime::{Gc, Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::lib_io;
use std::io::Read;

pub(crate) fn open_os_io(vm: &mut Vm) {
    let os = vm.heap.new_table();
    for (name, f) in [
        ("clock", os_clock as crate::runtime::value::NativeFn),
        ("date", os_date),
        ("difftime", os_difftime),
        ("execute", os_execute),
        ("exit", os_exit),
        ("getenv", os_getenv),
        ("remove", os_remove),
        ("rename", os_rename),
        ("time", os_time),
        ("tmpname", os_tmpname),
    ] {
        let fv = vm.native(f);
        set_field(vm, os, name, fv);
    }
    // the locale in effect per category, which `os.setlocale` reads and sets
    let locale = vm.heap.new_table();
    for i in 1..=LC_COUNT {
        let c = Value::Str(vm.heap.intern(b"C"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { locale.as_mut() }
            .set(&mut vm.heap, Value::Int(i as i64), c)
            .expect("valid key");
    }
    vm.barrier_back_table(locale);
    let sl = vm.native_with(os_setlocale, Box::new([Value::Table(locale)]));
    set_field(vm, os, "setlocale", sl);
    vm.set_global("os", Value::Table(os))
        .expect("stdlib registration");
    vm.barrier_back_table(os);

    lib_io::open_io(vm);

    let f = vm.native(nat_loadfile);
    vm.set_global("loadfile", f).expect("stdlib registration");
    let f = vm.native(nat_dofile);
    vm.set_global("dofile", f).expect("stdlib registration");
}

fn set_field(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let k = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, v)
        .expect("valid key");
}

// ---- calendar ----

/// Howard Hinnant's days-from-civil: days since 1970-01-01 of the
/// proleptic Gregorian (y, m, d), `m` in 1..=12.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400; // [0, 399]
    let mm = m as i64;
    let doy = (153 * (mm + if mm > 2 { -3 } else { 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of `days_from_civil`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 {
        z / 146097
    } else {
        (z - 146096) / 146097
    };
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// A `struct tm` for UTC: `month` 1-based, `wday` 0 = Sunday, `yday`
/// 0-based, as C keeps them.
struct Tm {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    wday: u32,
    yday: u32,
}

/// `gmtime`: `None` when the year does not fit `tm_year` (an `int`).
fn gmtime(t: i64) -> Option<Tm> {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    i32::try_from(year - 1900).ok()?;
    let yday = (days - days_from_civil(year, 1, 1)) as u32;
    Some(Tm {
        year,
        month,
        day,
        hour: secs / 3600,
        min: (secs % 3600) / 60,
        sec: secs % 60,
        // 1970-01-01 was a Thursday
        wday: (days + 4).rem_euclid(7) as u32,
        yday,
    })
}

/// `mktime` over UTC: normalise the out-of-range fields and return the
/// time, or `None` when it cannot be represented. A result of -1 is a
/// failure to `mktime`'s callers too.
fn mktime(year: i64, mon0: i64, mday: i64, hour: i64, min: i64, sec: i64) -> Option<i64> {
    let year = year.checked_add(mon0.div_euclid(12))?;
    let month = mon0.rem_euclid(12) as u32 + 1;
    let t = days_from_civil(year, month, 1)
        .checked_add(mday - 1)?
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + min * 60 + sec)?;
    gmtime(t)?;
    (t != -1).then_some(t)
}

/// ISO 8601 week-based year and week (for `%G`, `%g`, `%V`).
fn iso_week(tm: &Tm) -> (i64, u32) {
    let wd = (tm.wday + 6) % 7; // Monday = 0
    let week = (tm.yday as i64 - wd as i64 + 10) / 7;
    if week < 1 {
        // the last week of the previous year
        let py = tm.year - 1;
        let pyday = tm.yday as i64 + if is_leap(py) { 366 } else { 365 };
        let pwd = wd as i64;
        return (py, ((pyday - pwd + 10) / 7) as u32);
    }
    let days_in_year = if is_leap(tm.year) { 366 } else { 365 };
    // a week of which 4+ days fall in January belongs to the next year
    if week == 53 && tm.yday as i64 - wd as i64 + 3 >= days_in_year {
        return (tm.year + 1, 1);
    }
    (tm.year, week as u32)
}

// ---- os.time / os.date ----

fn now() -> i64 {
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
fn setallfields(vm: &mut Vm, t: Gc<Table>, tm: &Tm) -> Result<(), LuaError> {
    let fields = [
        ("year", tm.year),
        ("month", tm.month as i64),
        ("day", tm.day as i64),
        ("hour", tm.hour as i64),
        ("min", tm.min as i64),
        ("sec", tm.sec as i64),
        ("yday", tm.yday as i64 + 1),
        ("wday", tm.wday as i64 + 1),
    ];
    // ≤5.3 set them from seconds upwards
    let order: [usize; 8] = if vm.version() <= LuaVersion::Lua53 {
        [5, 4, 3, 2, 1, 0, 7, 6]
    } else {
        [0, 1, 2, 3, 4, 5, 6, 7]
    };
    for i in order {
        setfield(vm, t, fields[i].0, Value::Int(fields[i].1))?;
    }
    // UTC has no daylight saving time
    setfield(vm, t, "isdst", Value::Bool(false))
}

fn os_time(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    // isdst is read (with its metamethods) but UTC has no DST to apply
    let k = Value::Str(vm.heap.intern(b"isdst"));
    vm.index_value(Value::Table(t), k)?;
    let r = mktime(
        year as i64 + 1900,
        mon as i64,
        mday as i64,
        hour as i64,
        min as i64,
        sec as i64,
    );
    let Some(secs) = r else {
        if v <= LuaVersion::Lua52 {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        }
        return Err(raise_str(
            vm,
            "time result cannot be represented in this installation",
        ));
    };
    // 5.3+ write the normalised fields back
    if v >= LuaVersion::Lua53 {
        let tm = gmtime(secs).expect("mktime checked the year fits");
        setallfields(vm, t, &tm)?;
    }
    let r = time_value(vm, secs);
    Ok(vm.nat_return(fs, &[r]))
}

/// `l_checktime` (5.3+) or ≤5.2's `(time_t)luaL_checknumber`.
fn check_time(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    if vm.version() <= LuaVersion::Lua52 {
        return Ok(argcheck::check_number(vm, a, i)? as i64);
    }
    argcheck::check_integer(vm, a, i)
}

fn os_date(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let fmt = match argcheck::opt_string(vm, a, 0)? {
        Some(s) => s.as_bytes().to_vec(),
        None => b"%c".to_vec(),
    };
    let t = if a.is_none_or_nil(vm, 1) {
        now()
    } else {
        check_time(vm, a, 1)?
    };
    // ≤5.2 walk the format as a C string; 5.3 honours its full length
    let fmt: &[u8] = if v <= LuaVersion::Lua52 {
        lib_io::c_str(&fmt)
    } else {
        &fmt
    };
    let fmt = fmt.strip_prefix(b"!").unwrap_or(fmt);
    let Some(tm) = gmtime(t) else {
        return match v {
            LuaVersion::Lua51 | LuaVersion::Lua52 => Ok(vm.nat_return(fs, &[Value::Nil])),
            LuaVersion::Lua53 => Err(raise_str(
                vm,
                "time result cannot be represented in this installation",
            )),
            _ => Err(raise_str(
                vm,
                "date result cannot be represented in this installation",
            )),
        };
    };
    if lib_io::c_str(fmt) == b"*t" {
        let table = vm.heap.new_table();
        setallfields(vm, table, &tm)?;
        return Ok(vm.nat_return(fs, &[Value::Table(table)]));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        let rest = &fmt[i + 1..];
        let len = match conversion_len(v, rest) {
            Some(n) => n,
            None if v == LuaVersion::Lua51 => {
                // 5.1 passes a lone trailing '%' through
                out.push(b'%');
                break;
            }
            None => {
                let shown = String::from_utf8_lossy(lib_io::c_str(rest)).into_owned();
                let msg = format!("invalid conversion specifier '%{shown}'");
                return Err(arg_error(vm, 1, &msg));
            }
        };
        strftime(&rest[..len], &tm, &mut out);
        i += 1 + len;
    }
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

/// How many bytes of `rest` (what follows a '%') form a conversion, or
/// `None` if they form none. 5.1 hands every '%' plus the next byte to
/// strftime unchecked; 5.2+ accept the C99 set, where the `E` and `O`
/// modifiers go with some letters only.
fn conversion_len(v: LuaVersion, rest: &[u8]) -> Option<usize> {
    let c = *rest.first()?;
    if v == LuaVersion::Lua51 {
        return Some(1);
    }
    if b"aAbBcCdDeFgGhHIjmMnprRStTuUVwWxXyYzZ%".contains(&c) {
        return Some(1);
    }
    let second = *rest.get(1)?;
    let ok = match c {
        b'E' => b"cCxXyY".contains(&second),
        b'O' => b"deHImMSuUVwWy".contains(&second),
        _ => false,
    };
    ok.then_some(2)
}

const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// BSD `_yconv`: a year split into a century part (`%C`) and a two-digit
/// part (`%y`) the way `%Y` prints them together.
fn yconv(year: i64, top: bool, yy: bool, out: &mut Vec<u8>) {
    // tm_year and its base 1900 are split separately so neither overflows
    let a = year - 1900;
    let mut trail = a % 100;
    let mut lead = a / 100 + 19;
    if trail < 0 && lead > 0 {
        trail += 100;
        lead -= 1;
    } else if lead < 0 && trail > 0 {
        trail -= 100;
        lead += 1;
    }
    if top {
        if lead == 0 && trail < 0 {
            out.extend_from_slice(b"-0");
        } else {
            out.extend_from_slice(format!("{lead:02}").as_bytes());
        }
    }
    if yy {
        out.extend_from_slice(format!("{:02}", trail.abs()).as_bytes());
    }
}

/// C-locale `strftime` of one conversion (`spec` without the '%') on a
/// UTC time, as the BSD libc PUC runs on formats it. `E`/`O` modifiers
/// change nothing in the C locale; an unknown conversion prints itself.
fn strftime(spec: &[u8], tm: &Tm, out: &mut Vec<u8>) {
    let c = match spec {
        [b'E' | b'O', c] => *c,
        [c] => *c,
        _ => unreachable!("conversions are one or two bytes"),
    };
    let mut put = |s: String| out.extend_from_slice(s.as_bytes());
    let h12 = if tm.hour.is_multiple_of(12) {
        12
    } else {
        tm.hour % 12
    };
    match c {
        b'a' => put(DAY_NAMES[tm.wday as usize][..3].to_string()),
        b'A' => put(DAY_NAMES[tm.wday as usize].to_string()),
        b'b' | b'h' => put(MONTH_NAMES[tm.month as usize - 1][..3].to_string()),
        b'B' => put(MONTH_NAMES[tm.month as usize - 1].to_string()),
        b'c' => {
            strftime(b"a", tm, out);
            out.push(b' ');
            strftime(b"b", tm, out);
            out.extend_from_slice(
                format!(" {:2} {:02}:{:02}:{:02} ", tm.day, tm.hour, tm.min, tm.sec).as_bytes(),
            );
            yconv(tm.year, true, true, out);
        }
        b'C' => yconv(tm.year, true, false, out),
        b'd' => put(format!("{:02}", tm.day)),
        b'D' | b'x' => {
            out.extend_from_slice(format!("{:02}/{:02}/", tm.month, tm.day).as_bytes());
            yconv(tm.year, false, true, out);
        }
        b'e' => put(format!("{:2}", tm.day)),
        b'F' => {
            yconv(tm.year, true, true, out);
            out.extend_from_slice(format!("-{:02}-{:02}", tm.month, tm.day).as_bytes());
        }
        b'g' => yconv(iso_week(tm).0, false, true, out),
        b'G' => yconv(iso_week(tm).0, true, true, out),
        b'H' => put(format!("{:02}", tm.hour)),
        b'I' => put(format!("{h12:02}")),
        b'j' => put(format!("{:03}", tm.yday + 1)),
        b'k' => put(format!("{:2}", tm.hour)),
        b'l' => put(format!("{h12:2}")),
        b'm' => put(format!("{:02}", tm.month)),
        b'M' => put(format!("{:02}", tm.min)),
        b'n' => put("\n".to_string()),
        b'p' => put(if tm.hour < 12 { "AM" } else { "PM" }.to_string()),
        b'r' => put(format!(
            "{h12:02}:{:02}:{:02} {}",
            tm.min,
            tm.sec,
            if tm.hour < 12 { "AM" } else { "PM" }
        )),
        b'R' => put(format!("{:02}:{:02}", tm.hour, tm.min)),
        b's' => {
            let days = days_from_civil(tm.year, tm.month, tm.day);
            put((days * 86_400 + (tm.hour * 3600 + tm.min * 60 + tm.sec) as i64).to_string());
        }
        b'S' => put(format!("{:02}", tm.sec)),
        b't' => put("\t".to_string()),
        b'T' | b'X' => put(format!("{:02}:{:02}:{:02}", tm.hour, tm.min, tm.sec)),
        b'u' => put(if tm.wday == 0 { 7 } else { tm.wday }.to_string()),
        b'U' => put(format!("{:02}", (tm.yday + 7 - tm.wday) / 7)),
        b'v' => {
            out.extend_from_slice(format!("{:2}-", tm.day).as_bytes());
            strftime(b"b", tm, out);
            out.push(b'-');
            yconv(tm.year, true, true, out);
        }
        b'V' => put(format!("{:02}", iso_week(tm).1)),
        b'w' => put(tm.wday.to_string()),
        b'W' => put(format!("{:02}", (tm.yday + 7 - (tm.wday + 6) % 7) / 7)),
        b'y' => yconv(tm.year, false, true, out),
        b'Y' => yconv(tm.year, true, true, out),
        b'z' => put("+0000".to_string()),
        b'Z' => put("UTC".to_string()),
        b'+' => {
            strftime(b"a", tm, out);
            out.push(b' ');
            strftime(b"b", tm, out);
            out.extend_from_slice(
                format!(
                    " {:2} {:02}:{:02}:{:02} UTC ",
                    tm.day, tm.hour, tm.min, tm.sec
                )
                .as_bytes(),
            );
            yconv(tm.year, true, true, out);
        }
        other => out.push(other),
    }
}

fn os_difftime(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

fn os_clock(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    // C's clock() is processor time, which std cannot read; the time since
    // the Vm started stands in for it.
    let secs = vm.uptime().as_secs_f64();
    Ok(vm.nat_return(fs, &[Value::Float(secs)]))
}

// ---- environment, files, processes ----

fn os_getenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let v = match std::env::var_os(lib_io::os_path(&name)) {
        Some(val) => Value::Str(vm.heap.intern(&os_bytes(&val))),
        None => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

/// C `remove`: `rmdir` for a directory, `unlink` otherwise.
fn remove_path(p: &std::path::Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(p)?.is_dir() {
        std::fs::remove_dir(p)
    } else {
        std::fs::remove_file(p)
    }
}

fn os_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    Ok(match remove_path(&lib_io::os_path(&name)) {
        Ok(()) => vm.nat_return(fs, &[Value::Bool(true)]),
        Err(e) => lib_io::file_fail(vm, fs, Some(&name), &e),
    })
}

fn os_rename(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let from = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let to = argcheck::check_string(vm, a, 1)?.as_bytes().to_vec();
    Ok(
        match std::fs::rename(lib_io::os_path(&from), lib_io::os_path(&to)) {
            Ok(()) => vm.nat_return(fs, &[Value::Bool(true)]),
            // 5.1 names the source file in the message; 5.2+ name nothing
            Err(e) => {
                let fname = (vm.version() == LuaVersion::Lua51).then_some(from.as_slice());
                lib_io::file_fail(vm, fs, fname, &e)
            }
        },
    )
}

/// `os.tmpname`: POSIX builds use `mkstemp("/tmp/lua_XXXXXX")`, which
/// creates the file it names.
fn os_tmpname(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::hash::{BuildHasher, Hasher};
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let dir = if cfg!(unix) {
        std::path::PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    // mkstemp's own retry budget
    for _ in 0..100 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .expect("the clock reads after 1970"),
        );
        let mut bits = h.finish();
        let name: String = (0..6)
            .map(|_| {
                let c = CHARS[(bits % CHARS.len() as u64) as usize] as char;
                bits /= CHARS.len() as u64;
                c
            })
            .collect();
        let path = dir.join(format!("lua_{name}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {
                let s = Value::Str(vm.heap.intern(os_bytes(path.as_os_str()).as_slice()));
                return Ok(vm.nat_return(fs, &[s]));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    Err(raise_str(vm, "unable to generate a unique filename"))
}

/// `os.execute([command])`: `system(3)`.
#[cfg(any(unix, windows))]
fn os_execute(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let cmd = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?.map(|s| s.as_bytes().to_vec());
    let v = vm.version();
    let Some(cmd) = cmd else {
        // system(NULL): whether a shell exists
        return Ok(if v == LuaVersion::Lua51 {
            vm.nat_return(fs, &[Value::Int(1)])
        } else {
            vm.nat_return(fs, &[Value::Bool(true)])
        });
    };
    let status = lib_io::shell_command(&cmd).status();
    if v == LuaVersion::Lua51 {
        // 5.1 returns system()'s raw wait status
        let raw = match status {
            Ok(s) => raw_wait_status(&s),
            Err(_) => -1,
        };
        return Ok(vm.nat_return(fs, &[Value::Int(raw as i64)]));
    }
    Ok(lib_io::exec_result(vm, fs, status))
}

#[cfg(unix)]
fn raw_wait_status(s: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.into_raw()
}

#[cfg(windows)]
fn raw_wait_status(s: &std::process::ExitStatus) -> i32 {
    s.code().expect("a Windows process always has an exit code")
}

/// Targets without processes (`wasm32-wasip1`): no shell; a command fails
/// the way `system` does when it cannot run one.
#[cfg(not(any(unix, windows)))]
fn os_execute(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let cmd = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?;
    let v = vm.version();
    if cmd.is_none() {
        return Ok(if v == LuaVersion::Lua51 {
            vm.nat_return(fs, &[Value::Int(0)])
        } else {
            vm.nat_return(fs, &[Value::Bool(false)])
        });
    }
    if v == LuaVersion::Lua51 {
        return Ok(vm.nat_return(fs, &[Value::Int(-1)]));
    }
    let kind = Value::Str(vm.heap.intern(b"exit"));
    Ok(vm.nat_return(fs, &[Value::Nil, kind, Value::Int(-1)]))
}

/// `os.exit([code [, close]])`. Like C `exit`, every stream's pending
/// output is written first; with `close` (5.2+) the state is closed too,
/// running its finalizers.
fn os_exit(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let code = match a.get(vm, 0) {
        Value::Bool(b) if vm.version() >= LuaVersion::Lua52 && !a.is_none(0) => {
            if b {
                0
            } else {
                1
            }
        }
        _ => argcheck::opt_integer(vm, a, 0, 0)? as i32,
    };
    if vm.version() >= LuaVersion::Lua52 && a.get(vm, 1).truthy() {
        vm.close_state();
    }
    lib_io::flush_all(vm);
    std::process::exit(code);
}

/// Locale categories `os.setlocale` names, and the one more that "all"
/// covers (LC_MESSAGES), in the order a mixed query lists them.
const LC_COUNT: usize = 6;
const LC_NAMES: [&str; 6] = ["all", "collate", "ctype", "monetary", "numeric", "time"];

/// `os.setlocale([locale [, category]])`. luna has the C locale only, which
/// is also known as "POSIX"; "" selects the one the environment names.
fn os_setlocale(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let name = argcheck::opt_string(vm, a, 0)?.map(|s| lib_io::c_str(s.as_bytes()).to_vec());
    let cat = argcheck::check_option(vm, a, 1, Some("all"), &LC_NAMES)?;
    let Value::Table(state) = vm.nat_upval(fs, 0) else {
        unreachable!("setlocale's upvalue is its state table");
    };
    // table slots 1..=6: collate, ctype, monetary, numeric, time, messages
    let slots: Vec<i64> = if cat == 0 {
        (1..=LC_COUNT as i64).collect()
    } else {
        vec![cat as i64]
    };
    if let Some(name) = name {
        let resolved = if name.is_empty() {
            env_locale(LC_NAMES[cat])
        } else {
            name
        };
        if resolved != b"C" && resolved != b"POSIX" {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        }
        let v = Value::Str(vm.heap.intern(&resolved));
        for &i in &slots {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { state.as_mut() }
                .set(&mut vm.heap, Value::Int(i), v)
                .expect("valid key");
        }
        vm.barrier_back_table(state);
    }
    let names: Vec<Vec<u8>> = slots
        .iter()
        .map(|&i| match state.get(Value::Int(i)) {
            Value::Str(s) => s.as_bytes().to_vec(),
            _ => unreachable!("every category holds a locale name"),
        })
        .collect();
    // a mixed "all" reads as the categories joined by '/'
    let text = if names.iter().all(|n| *n == names[0]) {
        names[0].clone()
    } else {
        names.join(&b'/')
    };
    let r = Value::Str(vm.heap.intern(&text));
    Ok(vm.nat_return(fs, &[r]))
}

/// The locale `setlocale(cat, "")` picks: LC_ALL, then the category's own
/// variable, then LANG, else "C".
fn env_locale(cat: &str) -> Vec<u8> {
    let own = format!("LC_{}", cat.to_ascii_uppercase());
    for var in ["LC_ALL", own.as_str(), "LANG"] {
        if let Some(v) = std::env::var_os(var)
            && !v.is_empty()
        {
            return os_bytes(&v);
        }
    }
    b"C".to_vec()
}

// ---- loadfile / dofile ----

/// PUC `luaL_loadfilex`: compile the file `name` (stdin when `None`) and
/// return the function, or the message `loadfile` returns after its nil.
/// `mode` limits the chunk to text and/or binary (`None` allows both).
fn load_path(vm: &mut Vm, name: Option<&[u8]>, mode: Option<&[u8]>) -> Result<Value, Value> {
    let (read, chunkname) = match name {
        Some(n) => {
            let mut chunkname = vec![b'@'];
            chunkname.extend_from_slice(n);
            (
                std::fs::read(String::from_utf8_lossy(n).as_ref()),
                chunkname,
            )
        }
        None => {
            let mut buf = Vec::new();
            let r = std::io::stdin().read_to_end(&mut buf).map(|_| buf);
            (r, b"=stdin".to_vec())
        }
    };
    // `errfile`: the name shown is the chunk name without its '@' / '='.
    let shown = String::from_utf8_lossy(&chunkname[1..]).into_owned();
    let src = match read {
        Ok(src) => src,
        Err(e) => {
            let msg = format!("cannot open {shown}: {}", os_error_text(&e));
            return Err(Value::Str(vm.heap.intern(msg.as_bytes())));
        }
    };
    let src = crate::frontend::lexer::Lexer::strip_shebang_bom(&src);
    // PUC `luaL_loadfilex`: when a `#` comment line precedes a binary
    // chunk, the leading line-terminator left by the comment skip is
    // dropped so undump sees a clean `\x1bLua…` head (files.lua :594).
    let src: &[u8] = match src {
        [b'\n', rest @ ..] | [b'\r', b'\n', rest @ ..] | [b'\r', rest @ ..]
            if rest.first() == Some(&0x1b) =>
        {
            rest
        }
        _ => src,
    };
    // `checkmode` (ldo.c): the kind of chunk must be allowed by the mode.
    let binary = crate::vm::dump::is_binary_chunk(src);
    if let Some(mode) = mode
        && !mode.contains(if binary { &b'b' } else { &b't' })
    {
        let kind = if binary { "binary" } else { "text" };
        let msg = format!(
            "attempt to load a {kind} chunk (mode is '{}')",
            String::from_utf8_lossy(mode)
        );
        return Err(Value::Str(vm.heap.intern(msg.as_bytes())));
    }
    match vm.load(src, &chunkname) {
        Ok(cl) => Ok(Value::Closure(cl)),
        Err(e) => {
            // the parser positions its message with `luaO_chunkid`
            let mut msg = crate::vm::lib_debug::chunk_id(&chunkname);
            msg.extend_from_slice(format!(":{}: ", e.line).as_bytes());
            msg.extend_from_slice(&e.msg);
            Err(Value::Str(vm.heap.intern(&msg)))
        }
    }
}

/// C `strerror` for an OS error: Rust renders it as
/// "<strerror text> (os error N)"; PUC prints the text alone.
fn os_error_text(e: &std::io::Error) -> String {
    let full = e.to_string();
    match (e.raw_os_error(), full.rfind(" (os error ")) {
        (Some(_), Some(at)) => full[..at].to_string(),
        _ => full,
    }
}

/// `loadfile([filename [, mode [, env]]])`; 5.1 takes the filename only.
pub(crate) fn nat_loadfile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::version::LuaVersion;
    use crate::vm::argcheck::{self, Args};
    let a = Args::new(fs, nargs);
    let name = argcheck::opt_string(vm, a, 0)?;
    let mode = if vm.version() >= LuaVersion::Lua52 {
        argcheck::opt_string(vm, a, 1)?
    } else {
        None
    };
    // 5.5 `getMode`: Lua code cannot ask for a fixed-buffer ('B') chunk.
    if vm.version() >= LuaVersion::Lua55 && mode.is_some_and(|m| m.as_bytes().contains(&b'B')) {
        return Err(arg_error(vm, 2, "invalid mode"));
    }
    match load_path(
        vm,
        name.as_ref().map(|n| n.as_bytes()),
        mode.as_ref().map(|m| m.as_bytes()),
    ) {
        Ok(Value::Closure(cl)) => {
            // `load_aux`: a given env (even nil) becomes the first upvalue,
            // when the function has one.
            if vm.version() >= LuaVersion::Lua52 && !a.is_none(2) && !cl.upvals().is_empty() {
                let env = a.get(vm, 2);
                let uv = vm.heap.new_upvalue(crate::runtime::UpvalState::Closed(env));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { cl.as_mut() }.upvals_mut()[0] = uv;
            }
            Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
        }
        Ok(other) => Ok(vm.nat_return(fs, &[other])),
        Err(msg) => Ok(vm.nat_return(fs, &[Value::Nil, msg])),
    }
}

/// `dofile([filename])`: a load failure is raised as is (`lua_error`, no
/// position added).
fn nat_dofile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::argcheck::{self, Args};
    let name = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?;
    match load_path(vm, name.as_ref().map(|n| n.as_bytes()), None) {
        Ok(f) => {
            let results = vm.call_value(f, &[])?;
            Ok(vm.nat_return(fs, &results))
        }
        Err(msg) => Err(LuaError(msg)),
    }
}
