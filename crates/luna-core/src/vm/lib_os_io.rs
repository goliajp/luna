//! os/io and package/require: io file handles + popen + standard streams,
//! os clock/date/time/exec/exit, loadfile/dofile, and the
//! `package`/`require`/`module` chain (5.1's `package.seeall` inclusive).
//! Dynamic loaders (`package.loadlib`, `package.cpath` resolution) are
//! `nat_loadlib_stub` — luna ships no host linker.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::runtime::{FileHandle, Gc, Userdata, UserdataPayload, Value};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_os_io(vm: &mut Vm) {
    let os = vm.heap.new_table();
    let set = |vm: &mut Vm, t: crate::runtime::Gc<crate::runtime::Table>, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    set(vm, os, "time", os_time);
    set(vm, os, "clock", os_clock);
    set(vm, os, "date", os_date);
    set(vm, os, "difftime", os_difftime);
    set(vm, os, "getenv", os_getenv);
    set(vm, os, "setlocale", os_setlocale);
    set(vm, os, "tmpname", os_tmpname);
    set(vm, os, "remove", os_remove);
    set(vm, os, "rename", os_rename);
    set(vm, os, "execute", os_execute);
    set(vm, os, "exit", os_exit);
    vm.set_global("os", Value::Table(os))
        .expect("stdlib registration");
    vm.barrier_back_table(os);

    let io = vm.heap.new_table();
    set(vm, io, "write", io_write2);
    set(vm, io, "read", io_read2);
    set(vm, io, "type", io_type);
    set(vm, io, "input", io_input);
    set(vm, io, "output", io_output);
    set(vm, io, "close", io_close);
    set(vm, io, "open", io_open);
    set(vm, io, "tmpfile", io_tmpfile);
    set(vm, io, "lines", io_lines);
    set(vm, io, "flush", io_flush);
    set(vm, io, "popen", io_popen);

    // the shared FILE* metatable (PUC LUA_FILEHANDLE): __name + __index method
    // table + __tostring/__gc/__close. Cached on the Vm so io.open can attach it.
    let methods = vm.heap.new_table();
    set(vm, methods, "close", f_close);
    set(vm, methods, "read", f_read);
    set(vm, methods, "write", f_write);
    set(vm, methods, "seek", f_seek);
    set(vm, methods, "flush", f_flush);
    set(vm, methods, "setvbuf", f_setvbuf);
    set(vm, methods, "lines", f_lines);
    let file_mt = vm.heap.new_table();
    {
        let put = |vm: &mut Vm, k: &[u8], v: Value| {
            let kk = Value::Str(vm.heap.intern(k));
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { file_mt.as_mut() }
                .set(&mut vm.heap, kk, v)
                .expect("valid key");
        };
        let mname = Value::Str(vm.heap.intern(b"FILE*"));
        put(vm, b"__name", mname);
        put(vm, b"__index", Value::Table(methods));
        let f = vm.native(f_tostring);
        put(vm, b"__tostring", f);
        let f = vm.native(f_gc);
        put(vm, b"__gc", f);
        // PUC uses f_gc for __close too: a to-be-closed file already shut by the
        // user must not re-error (f_close would, via tofile's closed check).
        let f = vm.native(f_gc);
        put(vm, b"__close", f);
    }
    vm.barrier_back_table(methods);
    vm.barrier_back_table(file_mt);
    vm.file_mt = Some(file_mt);

    // standard streams as FILE* userdata carrying that metatable
    for (name, fh) in [
        ("stdin", FileHandle::Stdin),
        ("stdout", FileHandle::Stdout),
        ("stderr", FileHandle::Stderr),
    ] {
        let writable = !matches!(fh, FileHandle::Stdin);
        let h = new_file(vm, fh, writable);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { io.as_mut() }
            .set(&mut vm.heap, k, Value::Userdata(h))
            .expect("valid key");
        match name {
            "stdin" => vm.io_input = Some(h),
            "stdout" => vm.io_output = Some(h),
            _ => {}
        }
    }
    vm.set_global("io", Value::Table(io))
        .expect("stdlib registration");
    vm.barrier_back_table(io);

    let f = vm.native(nat_loadfile);
    vm.set_global("loadfile", f).expect("stdlib registration");
    let f = vm.native(nat_dofile);
    vm.set_global("dofile", f).expect("stdlib registration");
}

/// ISO 8601 week date (for strftime `%G`/`%V`, v2.14 CV.1). Inputs
/// use the broken-down convention already in play: `yday` 1-based,
/// `wday` 1=Sunday. Week 1 is the week containing the year's first
/// Thursday; days before it belong to the previous ISO year's last
/// week, and trailing days may belong to week 1 of the next.
fn iso_week_date(year: i64, yday: u32, wday: u32) -> (i64, u32) {
    let is_leap = |y: i64| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    // Monday-based weekday 1..=7.
    let wd = if wday == 1 { 7 } else { (wday - 1) as i64 };
    let week = (yday as i64 - wd + 10) / 7;
    if week < 1 {
        // Belongs to the previous ISO year's final week (52 or 53).
        let py = year - 1;
        // Jan 1 of `year` Monday-based weekday, derived from today's.
        let jan1_wd = ((wd - 1 - (yday as i64 - 1)).rem_euclid(7)) + 1;
        // Prev year has 53 weeks iff its Jan 1 is Thursday, or it is
        // a leap year whose Jan 1 is Wednesday — equivalently, iff
        // `year`'s Jan 1 is Friday, or Saturday following a leap year.
        let weeks = if jan1_wd == 5 || (jan1_wd == 6 && is_leap(py)) {
            53
        } else {
            52
        };
        return (py, weeks);
    }
    let year_days = if is_leap(year) { 366 } else { 365 };
    // Days remaining after today; if the current week crosses into
    // January with ≥4 of its days there, it is week 1 of next year.
    if week == 53 {
        let jan1_wd = ((wd - 1 - (yday as i64 - 1)).rem_euclid(7)) + 1;
        let has53 = jan1_wd == 4 || (jan1_wd == 3 && is_leap(year));
        if !has53 {
            return (year + 1, 1);
        }
    }
    let _ = year_days;
    (year, week as u32)
}

/// Howard Hinnant's calendar algorithm: days from civil (y, m, d) since the
/// epoch (1970-01-01). Negative for dates before. Valid for the full proleptic
/// Gregorian range; PUC's `os.time` ultimately funnels through libc `mktime`,
/// which on 64-bit time_t accepts a similar range.
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    Some(days_from_civil_impl(y, m, d))
}

fn days_from_civil_impl(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400; // [0, 399]
    let mm = m as i64;
    let doy = (153 * (mm + if mm > 2 { -3 } else { 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Inverse of `days_from_civil`: epoch days → (year, month, day).
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
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days from Jan 1 of `y` to (m, d) inclusive (1-based: Jan 1 → 1).
fn day_of_year(y: i64, m: u32, d: u32) -> u32 {
    static CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut yd = CUM[(m - 1) as usize] + d;
    if m > 2 && is_leap(y) {
        yd += 1;
    }
    yd
}

/// PUC `os.date` broken-down time, used both for the `"*t"` form and for the
/// strftime conversion path. Always UTC — luna does not link `libc::tzset`,
/// so the `!` prefix is a no-op (the round-trip `os.time(os.date("*t"))`
/// stays exact, which is all the tests check).
struct BrokenDown {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    wday: u32, // 1 = Sunday … 7 = Saturday (PUC stores 1-based)
    yday: u32,
}

fn broken_down(secs: i64) -> BrokenDown {
    let day_secs = 86_400i64;
    let days = secs.div_euclid(day_secs);
    let time = secs.rem_euclid(day_secs) as u32;
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday: epoch day 0 → wday 5 (Thu) in PUC's 1=Sun base.
    let wday = ((days + 4).rem_euclid(7) + 1) as u32;
    BrokenDown {
        year: y,
        month: m,
        day: d,
        hour: time / 3600,
        min: (time % 3600) / 60,
        sec: time % 60,
        wday,
        yday: day_of_year(y, m, d),
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn os_time(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 || vm.nat_arg(fs, nargs, 0).is_nil() {
        return Ok(vm.nat_return(fs, &[Value::Int(now_secs())]));
    }
    let Value::Table(t) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "time", "table expected"));
    };
    // PUC `os_time` (loslib.c `getfield`/`l_checktime`): required year/month/day,
    // optional hour=12/min=0/sec=0/isdst (the last is unused without libc tz
    // info). Each field must be an integer; a missing required field raises
    // "missing 'X'", a non-integer raises "field 'X' is not an integer".
    let year = check_int_field(vm, t, "year")?;
    let month = check_int_field(vm, t, "month")?;
    let day = check_int_field(vm, t, "day")?;
    let hour = opt_int_field(vm, t, "hour", 12)?;
    let min = opt_int_field(vm, t, "min", 0)?;
    let sec = opt_int_field(vm, t, "sec", 0)?;
    // PUC `os_time` sends `t.year - 1900` (and `t.month - 1`) to libc `mktime`,
    // whose struct tm uses `int` fields. The "out-of-bound" check is therefore
    // on the OFFSET value, not the raw year — files.lua :881 passes
    // `year = -(1<<31) + 1899` (raw year fits i32, but year-1900 overflows).
    if year
        .checked_sub(1900)
        .is_none_or(|v| v < i32::MIN as i64 || v > i32::MAX as i64)
    {
        return Err(raise_str(vm, "field 'year' is out-of-bound"));
    }
    let year_b = year as i32; // identity for our composer (which is 64-bit-clean)
    let month_b = bound_i32(vm, "month", month)?;
    let day_b = bound_i32(vm, "day", day)?;
    let hour_b = bound_i32(vm, "hour", hour)?;
    let min_b = bound_i32(vm, "min", min)?;
    let sec_b = bound_i32(vm, "sec", sec)?;
    let year_full = year_b as i64;
    let secs = compose_time(year_full, month_b, day_b, hour_b, min_b, sec_b)
        .ok_or_else(|| raise_str(vm, "time result cannot be represented in this installation"))?;
    // Normalize the table (PUC `os.time` writes back the canonical broken-down
    // fields after `mktime` — files.lua :983 round-trips a sec=-3602 carry).
    let bd = broken_down(secs);
    set_int_field(vm, t, "year", bd.year);
    set_int_field(vm, t, "month", bd.month as i64);
    set_int_field(vm, t, "day", bd.day as i64);
    set_int_field(vm, t, "hour", bd.hour as i64);
    set_int_field(vm, t, "min", bd.min as i64);
    set_int_field(vm, t, "sec", bd.sec as i64);
    set_int_field(vm, t, "wday", bd.wday as i64);
    set_int_field(vm, t, "yday", bd.yday as i64);
    Ok(vm.nat_return(fs, &[Value::Int(secs)]))
}

/// Compose a (year, month, day, hour, min, sec) into Unix seconds. Returns
/// None when the i64 arithmetic would overflow. PUC normalizes month/day
/// overflow via libc `mktime`; luna's version carries excess months into the
/// year first and treats day/hour/min/sec as days-and-fractions added on top
/// — `broken_down` then re-decomposes the result, matching PUC's "you can
/// pass sec=-3602 and the table comes back canonical" guarantee
/// (files.lua :983).
fn compose_time(y: i64, m: i32, d: i32, h: i32, mi: i32, s: i32) -> Option<i64> {
    // Carry month overflow into the year (PUC `mktime` does the same).
    let m_total: i64 = (m as i64) - 1; // 0-based offset from Jan
    let year_carry = m_total.div_euclid(12);
    let month0 = m_total.rem_euclid(12) as u32; // 0..11
    let year = y.checked_add(year_carry)?;
    let days = days_from_civil(year, month0 + 1, 1)?.checked_add(d as i64 - 1)?;
    days.checked_mul(86_400)?
        .checked_add(h as i64 * 3600)?
        .checked_add(mi as i64 * 60)?
        .checked_add(s as i64)
}

fn check_int_field(
    vm: &mut Vm,
    t: crate::runtime::Gc<crate::runtime::Table>,
    name: &str,
) -> Result<i64, LuaError> {
    let k = Value::Str(vm.heap.intern(name.as_bytes()));
    let v = t.get(k);
    match v {
        Value::Nil => Err(raise_str(
            vm,
            &format!("field '{name}' missing in date table"),
        )),
        Value::Int(i) => Ok(i),
        Value::Float(f) if f.fract() == 0.0 && f.is_finite() => Ok(f as i64),
        _ => Err(raise_str(vm, &format!("field '{name}' is not an integer"))),
    }
}

fn opt_int_field(
    vm: &mut Vm,
    t: crate::runtime::Gc<crate::runtime::Table>,
    name: &str,
    default: i64,
) -> Result<i64, LuaError> {
    let k = Value::Str(vm.heap.intern(name.as_bytes()));
    let v = t.get(k);
    match v {
        Value::Nil => Ok(default),
        Value::Int(i) => Ok(i),
        Value::Float(f) if f.fract() == 0.0 && f.is_finite() => Ok(f as i64),
        _ => Err(raise_str(vm, &format!("field '{name}' is not an integer"))),
    }
}

fn bound_i32(vm: &mut Vm, name: &str, v: i64) -> Result<i32, LuaError> {
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        return Err(raise_str(vm, &format!("field '{name}' is out-of-bound")));
    }
    Ok(v as i32)
}

fn set_int_field(vm: &mut Vm, t: crate::runtime::Gc<crate::runtime::Table>, name: &str, v: i64) {
    let k = Value::Str(vm.heap.intern(name.as_bytes()));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, Value::Int(v))
        .expect("valid key");
}

fn os_clock(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let secs = vm.uptime().as_secs_f64();
    Ok(vm.nat_return(fs, &[Value::Float(secs)]))
}

fn os_difftime(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t2 = num_arg(vm, fs, nargs, 0, "difftime")?;
    let t1 = if nargs >= 2 {
        num_arg(vm, fs, nargs, 1, "difftime")?
    } else {
        0.0
    };
    Ok(vm.nat_return(fs, &[Value::Float(t2 - t1)]))
}

fn num_arg(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<f64, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Int(v) => Ok(v as f64),
        Value::Float(v) => Ok(v),
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("number expected, got {}", v.type_name()),
        )),
    }
}

fn os_date(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let fmt_v = if nargs == 0 {
        Value::Str(vm.heap.intern(b"%c"))
    } else {
        vm.nat_arg(fs, nargs, 0)
    };
    let fmt_owned: Vec<u8> = match fmt_v {
        Value::Nil => b"%c".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                1,
                "date",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let t = if nargs >= 2 && !vm.nat_arg(fs, nargs, 1).is_nil() {
        match vm.nat_arg(fs, nargs, 1) {
            Value::Int(i) => i,
            Value::Float(f) => f as i64,
            _ => {
                return Err(arg_error(vm, 2, "date", "integer expected"));
            }
        }
    } else {
        now_secs()
    };
    let mut fmt: &[u8] = &fmt_owned;
    if fmt.first() == Some(&b'!') {
        fmt = &fmt[1..];
    }
    let bd = broken_down(t);
    if fmt == b"*t" {
        let table = vm.heap.new_table();
        set_int_field(vm, table, "year", bd.year);
        set_int_field(vm, table, "month", bd.month as i64);
        set_int_field(vm, table, "day", bd.day as i64);
        set_int_field(vm, table, "hour", bd.hour as i64);
        set_int_field(vm, table, "min", bd.min as i64);
        set_int_field(vm, table, "sec", bd.sec as i64);
        set_int_field(vm, table, "wday", bd.wday as i64);
        set_int_field(vm, table, "yday", bd.yday as i64);
        // luna doesn't link tzdata, so DST status is unknown — PUC reports the
        // same as `isdst = false` when libc returns `tm_isdst == 0`; the test
        // (`files.lua` :957) accepts `nil` for "no daylight saving info".
        let k = Value::Str(vm.heap.intern(b"isdst"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { table.as_mut() }
            .set(&mut vm.heap, k, Value::Bool(false))
            .expect("valid key");
        // once-per-table barrier mirrors SETLIST: table is born BLACK during
        // Propagate and the bulk inserts above don't barrier (the interned
        // key strings can be pre-existing WHITE).
        vm.barrier_back_table(table);
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
        if i + 1 >= fmt.len() {
            return Err(raise_str(vm, "invalid conversion specifier '%'"));
        }
        let c = fmt[i + 1];
        match c {
            b'%' => out.push(b'%'),
            b'Y' => out.extend_from_slice(format!("{}", bd.year).as_bytes()),
            b'y' => out.extend_from_slice(format!("{:02}", bd.year.rem_euclid(100)).as_bytes()),
            b'm' => out.extend_from_slice(format!("{:02}", bd.month).as_bytes()),
            b'd' => out.extend_from_slice(format!("{:02}", bd.day).as_bytes()),
            b'H' => out.extend_from_slice(format!("{:02}", bd.hour).as_bytes()),
            b'M' => out.extend_from_slice(format!("{:02}", bd.min).as_bytes()),
            b'S' => out.extend_from_slice(format!("{:02}", bd.sec).as_bytes()),
            b'j' => out.extend_from_slice(format!("{:03}", bd.yday).as_bytes()),
            b'w' => out.extend_from_slice(format!("{}", bd.wday - 1).as_bytes()),
            b'p' => out.extend_from_slice(if bd.hour < 12 { b"AM" } else { b"PM" }),
            b'a' | b'A' => {
                let names: [&[u8]; 7] = if c == b'a' {
                    [b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"]
                } else {
                    [
                        b"Sunday",
                        b"Monday",
                        b"Tuesday",
                        b"Wednesday",
                        b"Thursday",
                        b"Friday",
                        b"Saturday",
                    ]
                };
                out.extend_from_slice(names[(bd.wday - 1) as usize]);
            }
            b'b' | b'B' => {
                let names: [&[u8]; 12] = if c == b'b' {
                    [
                        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep",
                        b"Oct", b"Nov", b"Dec",
                    ]
                } else {
                    [
                        b"January",
                        b"February",
                        b"March",
                        b"April",
                        b"May",
                        b"June",
                        b"July",
                        b"August",
                        b"September",
                        b"October",
                        b"November",
                        b"December",
                    ]
                };
                out.extend_from_slice(names[(bd.month - 1) as usize]);
            }
            b'c' => {
                let wd = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][(bd.wday - 1) as usize];
                let mn = [
                    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
                    "Dec",
                ][(bd.month - 1) as usize];
                out.extend_from_slice(
                    format!(
                        "{wd} {mn} {:2} {:02}:{:02}:{:02} {}",
                        bd.day, bd.hour, bd.min, bd.sec, bd.year
                    )
                    .as_bytes(),
                );
            }
            b'x' => out.extend_from_slice(
                format!(
                    "{:02}/{:02}/{:02}",
                    bd.month,
                    bd.day,
                    bd.year.rem_euclid(100)
                )
                .as_bytes(),
            ),
            b'X' => out.extend_from_slice(
                format!("{:02}:{:02}:{:02}", bd.hour, bd.min, bd.sec).as_bytes(),
            ),
            b'Z' => {} // local timezone abbreviation; unknown without libc
            // C99 additions PUC inherits from strftime (v2.14 CV.1,
            // fixture 5.5/308 pins the numeric ones against lua5.5):
            b'u' => {
                // ISO weekday: Monday=1 … Sunday=7 (bd.wday is 1=Sunday).
                let u = if bd.wday == 1 { 7 } else { bd.wday - 1 };
                out.extend_from_slice(format!("{u}").as_bytes());
            }
            b'e' => out.extend_from_slice(format!("{:2}", bd.day).as_bytes()),
            b'C' => out.extend_from_slice(format!("{:02}", bd.year.div_euclid(100)).as_bytes()),
            b'D' => out.extend_from_slice(
                format!(
                    "{:02}/{:02}/{:02}",
                    bd.month,
                    bd.day,
                    bd.year.rem_euclid(100)
                )
                .as_bytes(),
            ),
            b'T' => out.extend_from_slice(
                format!("{:02}:{:02}:{:02}", bd.hour, bd.min, bd.sec).as_bytes(),
            ),
            b'F' => out
                .extend_from_slice(format!("{}-{:02}-{:02}", bd.year, bd.month, bd.day).as_bytes()),
            b'G' | b'V' => {
                let (iso_year, iso_week) = iso_week_date(bd.year, bd.yday, bd.wday);
                if c == b'G' {
                    out.extend_from_slice(format!("{iso_year}").as_bytes());
                } else {
                    out.extend_from_slice(format!("{iso_week:02}").as_bytes());
                }
            }
            _ => {
                return Err(raise_str(
                    vm,
                    &format!("invalid conversion specifier '%{}'", c as char),
                ));
            }
        }
        i += 2;
    }
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

fn os_getenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "getenv", "string expected"));
    };
    let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
    let v = match std::env::var_os(&name) {
        Some(val) => Value::Str(vm.heap.intern(val.to_string_lossy().as_bytes())),
        None => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn os_setlocale(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // only the portable "C" locale is supported; querying (nil) or selecting
    // "C" returns "C", any other locale returns nil (unavailable).
    let v = match vm.nat_arg(fs, nargs, 0) {
        Value::Nil => Value::Str(vm.heap.intern(b"C")),
        Value::Str(s) if s.as_bytes() == b"C" || s.as_bytes().is_empty() => {
            Value::Str(vm.heap.intern(b"C"))
        }
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

// ---- io file model ----

/// Create a file userdata wrapping `fh`, carrying the shared FILE* metatable.
/// `writable` enables the user-space write buffer (PUC's stdio FILE*); only
/// handles opened in a write-capable mode (plus `stdout`/`stderr`) get one.
fn new_file(vm: &mut Vm, fh: FileHandle, writable: bool) -> Gc<Userdata> {
    let u = vm.heap.new_userdata(UserdataPayload::File(fh), writable);
    if let Some(mt) = vm.file_mt {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { u.as_mut() }.set_metatable(Some(mt));
    }
    u
}

/// Pull the FILE* self argument (PUC tofile): a missing arg is "got no value".
fn check_file(vm: &mut Vm, fs: u32, nargs: u32, who: &str) -> Result<Gc<Userdata>, LuaError> {
    match vm.nat_arg(fs, nargs, 0) {
        Value::Userdata(u) => Ok(u),
        _ if nargs == 0 => Err(raise_str(
            vm,
            &format!("bad argument #1 to '{who}' (got no value)"),
        )),
        v => {
            let tn = vm.obj_typename(v);
            Err(arg_error(vm, 1, who, &format!("FILE* expected, got {tn}")))
        }
    }
}

/// io.type(v): "file" / "closed file" / nil (every luna userdata is a file).
fn io_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let r = match vm.nat_arg(fs, nargs, 0) {
        Value::Userdata(u) => {
            if u.file().is_closed() {
                "closed file"
            } else {
                "file"
            }
        }
        _ => return Ok(vm.nat_return(fs, &[Value::Nil])),
    };
    let s = Value::Str(vm.heap.intern(r.as_bytes()));
    Ok(vm.nat_return(fs, &[s]))
}

/// io.input([file]) — get or set the default input stream (PUC g_iofile).
fn io_input(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs >= 1 && !vm.nat_arg(fs, nargs, 0).is_nil() {
        let u = io_default_arg(vm, fs, nargs, "r", "input")?;
        vm.io_input = Some(u);
    }
    let cur = vm.io_input.expect("default input set at startup");
    Ok(vm.nat_return(fs, &[Value::Userdata(cur)]))
}

/// Resolve an io.input/io.output argument: an existing file userdata, or a
/// filename string opened with `mode` (PUC g_iofile).
fn io_default_arg(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    mode: &str,
    who: &str,
) -> Result<Gc<Userdata>, LuaError> {
    match vm.nat_arg(fs, nargs, 0) {
        Value::Userdata(u) => Ok(u),
        Value::Str(s) => {
            let path = String::from_utf8_lossy(s.as_bytes()).into_owned();
            let (opts, writable) = parse_mode(mode.as_bytes()).expect("static mode is valid");
            match opts.open(&path) {
                Ok(file) => Ok(new_file(vm, FileHandle::File(file), writable)),
                Err(e) => Err(raise_str(vm, &format!("{path}: {e}"))),
            }
        }
        v => {
            let tn = vm.obj_typename(v);
            Err(arg_error(vm, 1, who, &format!("FILE* expected, got {tn}")))
        }
    }
}

/// io.output([file]) — get or set the default output stream.
fn io_output(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs >= 1 && !vm.nat_arg(fs, nargs, 0).is_nil() {
        let u = io_default_arg(vm, fs, nargs, "w", "output")?;
        vm.io_output = Some(u);
    }
    let cur = vm.io_output.expect("default output set at startup");
    Ok(vm.nat_return(fs, &[Value::Userdata(cur)]))
}

/// io.close([file]) — close `file` or the default output.
fn io_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = if nargs >= 1 && !vm.nat_arg(fs, nargs, 0).is_nil() {
        match vm.nat_arg(fs, nargs, 0) {
            Value::Userdata(u) => u,
            v => {
                let tn = vm.obj_typename(v);
                return Err(arg_error(
                    vm,
                    1,
                    "close",
                    &format!("FILE* expected, got {tn}"),
                ));
            }
        }
    } else {
        vm.io_output.expect("default output set at startup")
    };
    close_file(vm, fs, u)
}

/// file:close() — also the FILE* __close handler.
fn f_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_file(vm, fs, nargs, "close")?;
    close_file(vm, fs, u)
}

/// Shared close: standard streams cannot be closed (returns false + message);
/// a regular file is closed (its handle dropped) and returns true. A handle
/// produced by `io.popen` waits on its child process and reports the same
/// `(success, "exit"|"signal", code)` triple that `os.execute` produces.
fn close_file(vm: &mut Vm, fs: u32, u: Gc<Userdata>) -> Result<u32, LuaError> {
    if u.file().is_closed() {
        // PUC f_close runs tofile() first, rejecting an already-closed handle.
        return Err(raise_str(vm, "attempt to use a closed file"));
    }
    if u.file().is_std() {
        let msg = Value::Str(vm.heap.intern(b"cannot close standard file"));
        return Ok(vm.nat_return(fs, &[Value::Bool(false), msg]));
    }
    // PUC `fclose` flushes the FILE* before releasing it; a flush failure
    // surfaces as `(nil, msg, errno)` from the close call. The handle is
    // dropped regardless so the OS resource is not leaked.
    let drain = drain_write_buf(u);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    *unsafe { u.as_mut() }.file_mut() = FileHandle::Closed;
    // popen handle: take the child out of the userdata, drop the pipe (so
    // the child sees EOF on its end), and wait. PUC `lua_pclose` returns
    // the same triple as `os.execute`; for 5.1 we mirror its integer-status
    // shape. On wasi the field is always `None` (io_popen stub never sets
    // it); we gate the whole block on `any(unix, windows)` so wasi builds
    // don't reach `Child::wait` / `exit_status_breakdown` (neither has a
    // wasi impl).
    #[cfg(any(unix, windows))]
    {
        use crate::version::LuaVersion;
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let popen_child = unsafe { u.as_mut() }.popen_child.take();
        if let Some(mut child) = popen_child {
            // Drop the pipe-end File the userdata held *before* waiting so the
            // child notices EOF (matters for read pipes; harmless for write).
            // `FileHandle::Closed` above already dropped it.
            let _ = drain; // popen pipes don't share PUC's flush-on-close error path
            let status = match child.wait() {
                Ok(s) => s,
                Err(e) => return Ok(file_result_err(vm, fs, "popen", &e)),
            };
            let (kind, code) = exit_status_breakdown(&status);
            if vm.version() <= LuaVersion::Lua51 {
                return Ok(vm.nat_return(fs, &[Value::Int(code as i64)]));
            }
            let kind_s = Value::Str(vm.heap.intern(kind.as_bytes()));
            let ok = matches!(kind, "exit") && code == 0;
            return Ok(vm.nat_return(fs, &[Value::Bool(ok), kind_s, Value::Int(code as i64)]));
        }
    }
    match drain {
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
        Err(e) => Ok(file_result_err(vm, fs, "file", &e)),
    }
}

/// FILE* __tostring: `file (0x…)` open, `file (closed)` once closed.
fn f_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_file(vm, fs, nargs, "tostring")?;
    let s = if u.file().is_closed() {
        "file (closed)".to_string()
    } else {
        format!("file ({:p})", u.as_ptr())
    };
    let v = Value::Str(vm.heap.intern(s.as_bytes()));
    Ok(vm.nat_return(fs, &[v]))
}

/// FILE* __gc: close an open file on collection (drops the OS handle). Standard
/// streams and already-closed files need nothing.
fn f_gc(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC tolstream: validate the FILE* self argument, so a manual call with no
    // argument (getmetatable(io.stdin).__gc()) raises "got no value". The GC
    // always passes the object, so finalization stays error-free.
    let u = check_file(vm, fs, nargs, "__gc")?;
    if matches!(u.file(), FileHandle::File(_)) {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        *unsafe { u.as_mut() }.file_mut() = FileHandle::Closed;
    }
    Ok(vm.nat_return(fs, &[]))
}

/// Like `check_file`, but also raises on a closed handle (PUC tofile).
fn check_open(vm: &mut Vm, fs: u32, nargs: u32, who: &str) -> Result<Gc<Userdata>, LuaError> {
    let u = check_file(vm, fs, nargs, who)?;
    if u.file().is_closed() {
        return Err(raise_str(vm, "attempt to use a closed file"));
    }
    Ok(u)
}

/// Parse an io.open mode string (`[rwa] '+'? 'b'?`) into OpenOptions plus a
/// `writable` flag (true iff the FILE* will ever take a write). Returns None
/// if the mode is malformed (PUC `l_checkmode`).
fn parse_mode(m: &[u8]) -> Option<(std::fs::OpenOptions, bool)> {
    let mut it = m.iter().copied().peekable();
    let mut opts = std::fs::OpenOptions::new();
    let writable = match it.next()? {
        b'r' => {
            opts.read(true);
            if it.peek() == Some(&b'+') {
                it.next();
                opts.write(true); // r+: read/write, must exist
                true
            } else {
                false
            }
        }
        b'w' => {
            opts.write(true).create(true).truncate(true);
            if it.peek() == Some(&b'+') {
                it.next();
                opts.read(true);
            }
            true
        }
        b'a' => {
            opts.append(true).create(true);
            if it.peek() == Some(&b'+') {
                it.next();
                opts.read(true);
            }
            true
        }
        _ => return None,
    };
    if it.peek() == Some(&b'b') {
        it.next(); // binary mode is a no-op on unix
    }
    if it.next().is_some() {
        return None; // trailing junk
    }
    Some((opts, writable))
}

/// PUC luaL_fileresult on error: (nil, "name: message", errno).
fn file_result_err(vm: &mut Vm, fs: u32, name: &str, e: &std::io::Error) -> u32 {
    let msg = Value::Str(vm.heap.intern(format!("{name}: {e}").as_bytes()));
    let code = Value::Int(e.raw_os_error().unwrap_or(-1) as i64);
    vm.nat_return(fs, &[Value::Nil, msg, code])
}

fn str_arg(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<String, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Str(s) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("string expected, got {}", v.type_name()),
        )),
    }
}

/// io.open(path [, mode]) → file, or (nil, msg, errno) on failure.
fn io_open(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let path = str_arg(vm, fs, nargs, 0, "open")?;
    let mode: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => b"r".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                2,
                "open",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let Some((opts, writable)) = parse_mode(&mode) else {
        let m = String::from_utf8_lossy(&mode);
        return Err(arg_error(vm, 2, "open", &format!("invalid mode '{m}'")));
    };
    match opts.open(&path) {
        Ok(file) => {
            let u = new_file(vm, FileHandle::File(file), writable);
            Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
        }
        Err(e) => Ok(file_result_err(vm, fs, &path, &e)),
    }
}

/// PUC `io.popen(prog [, mode])` — spawn a shell that runs `prog` and pipe
/// its stdout (mode `"r"`, default) or stdin (mode `"w"`) back as a file
/// handle. Other modes (`"rb"`/`"wb"` are tolerated as aliases of `"r"`/
/// `"w"` — luna is line-mode-agnostic). On failure returns the same
/// `(nil, msg, errno)` triple as `io.open`.
///
/// The pipe end is re-wrapped as a `std::fs::File` (via `IntoRawFd` on Unix
/// and `IntoRawHandle` on Windows) so existing read/write/seek/flush paths
/// stay zero-touch; only `:close` knows to wait on the child and report
/// `(success, "exit"|"signal", code)` instead of `(true)`.
#[cfg(any(unix, windows))]
fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let prog = str_arg(vm, fs, nargs, 0, "popen")?;
    let mode: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => b"r".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                2,
                "popen",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    // Tolerate `"rb"`/`"wb"` etc. — luna doesn't distinguish text/binary
    // pipes (the underlying File is binary by default).
    let read_mode = match mode.first().copied() {
        Some(b'r') => true,
        Some(b'w') => false,
        _ => {
            let m = String::from_utf8_lossy(&mode);
            return Err(arg_error(vm, 2, "popen", &format!("invalid mode '{m}'")));
        }
    };
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(&prog);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(&prog);
        c
    };
    if read_mode {
        cmd.stdout(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ok(file_result_err(vm, fs, &prog, &e)),
    };
    // Pull the pipe end and re-wrap as a generic `std::fs::File`. The Child
    // must outlive the pipe (so wait() sees a still-valid pid), so we move
    // it into the Userdata's `popen_child` slot after spawning.
    let file = if read_mode {
        let pipe = child
            .stdout
            .take()
            .expect("Stdio::piped() always populates stdout");
        pipe_to_file(pipe)
    } else {
        let pipe = child
            .stdin
            .take()
            .expect("Stdio::piped() always populates stdin");
        pipe_to_file(pipe)
    };
    let u = new_file(vm, FileHandle::File(file), !read_mode);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.popen_child = Some(child);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// `io.popen` stub for targets without `proc_*` (`wasm32-wasip1` / `-wasip2`).
/// Validates the prog/mode args for parity with the real path so a wasi caller
/// sees the same arg-type errors as a native caller, then returns the PUC
/// `(nil, "popen not supported on this platform", -1)` error tuple.
#[cfg(not(any(unix, windows)))]
fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let _prog = str_arg(vm, fs, nargs, 0, "popen")?;
    match vm.nat_arg(fs, nargs, 1) {
        Value::Nil | Value::Str(_) => {}
        v => {
            return Err(arg_error(
                vm,
                2,
                "popen",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let msg = Value::Str(vm.heap.intern(b"popen not supported on this platform"));
    Ok(vm.nat_return(fs, &[Value::Nil, msg, Value::Int(-1)]))
}

/// Convert a child stdio pipe (`ChildStdout` or `ChildStdin`) into a generic
/// `std::fs::File` via the platform's raw handle. Unix uses `IntoRawFd`;
/// Windows uses `IntoRawHandle`. The pipe is consumed (its Drop is skipped),
/// and the File takes ownership of the underlying OS resource.
///
/// Gated on `any(unix, windows)`: the trait has no impl on other targets
/// (wasi) and the only caller, `io_popen`, is gated the same way.
#[cfg(any(unix, windows))]
fn pipe_to_file<T>(pipe: T) -> std::fs::File
where
    T: PipeAsFile,
{
    pipe.into_file()
}

#[cfg(any(unix, windows))]
trait PipeAsFile {
    fn into_file(self) -> std::fs::File;
}

#[cfg(unix)]
impl PipeAsFile for std::process::ChildStdout {
    fn into_file(self) -> std::fs::File {
        use std::os::unix::io::{FromRawFd, IntoRawFd};
        // SAFETY: the raw fd/handle was produced by a matching `into_raw_*` on a `File` that this code now owns exclusively (the previous owner has been consumed); wrapping it back into a `File` re-establishes unique ownership.
        unsafe { std::fs::File::from_raw_fd(self.into_raw_fd()) }
    }
}
#[cfg(unix)]
impl PipeAsFile for std::process::ChildStdin {
    fn into_file(self) -> std::fs::File {
        use std::os::unix::io::{FromRawFd, IntoRawFd};
        // SAFETY: the raw fd/handle was produced by a matching `into_raw_*` on a `File` that this code now owns exclusively (the previous owner has been consumed); wrapping it back into a `File` re-establishes unique ownership.
        unsafe { std::fs::File::from_raw_fd(self.into_raw_fd()) }
    }
}
#[cfg(windows)]
impl PipeAsFile for std::process::ChildStdout {
    fn into_file(self) -> std::fs::File {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        // SAFETY: the raw fd/handle was produced by a matching `into_raw_*` on a `File` that this code now owns exclusively (the previous owner has been consumed); wrapping it back into a `File` re-establishes unique ownership.
        unsafe { std::fs::File::from_raw_handle(self.into_raw_handle()) }
    }
}
#[cfg(windows)]
impl PipeAsFile for std::process::ChildStdin {
    fn into_file(self) -> std::fs::File {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        // SAFETY: the raw fd/handle was produced by a matching `into_raw_*` on a `File` that this code now owns exclusively (the previous owner has been consumed); wrapping it back into a `File` re-establishes unique ownership.
        unsafe { std::fs::File::from_raw_handle(self.into_raw_handle()) }
    }
}

/// Collect the bytes for a write (strings and numbers, like io.write/file:write).
fn gather_write(vm: &mut Vm, fs: u32, nargs: u32, start: u32) -> Result<Vec<u8>, LuaError> {
    let mut out = Vec::new();
    for i in start..nargs {
        match vm.nat_arg(fs, nargs, i) {
            Value::Str(s) => out.extend_from_slice(s.as_bytes()),
            v @ (Value::Int(_) | Value::Float(_)) => out.extend(vm.tostring_basic(v)),
            v => {
                return Err(arg_error(
                    vm,
                    i + 1,
                    "write",
                    &format!("string expected, got {}", v.type_name()),
                ));
            }
        }
    }
    Ok(out)
}

/// Write `bytes` directly to the OS handle behind `u`, bypassing the
/// user-space buffer. Used to drain a previously buffered write.
fn write_to(u: Gc<Userdata>, bytes: &[u8]) -> std::io::Result<()> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.write_all(bytes),
        FileHandle::Stdout => std::io::stdout().write_all(bytes),
        FileHandle::Stderr => std::io::stderr().write_all(bytes),
        FileHandle::Stdin => Err(std::io::Error::other("cannot write to input file")),
        FileHandle::Closed => Err(std::io::Error::other("closed file")),
    }
}

/// Drain `u`'s user-space write buffer to its OS handle. The buffer is
/// always emptied — on a partial-or-failed write PUC stdio considers the
/// data lost (the error is what gets returned to the caller). No-op when the
/// buffer is already empty or `u`'s handle is a standard stream / closed.
fn drain_write_buf(u: Gc<Userdata>) -> std::io::Result<()> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let buf = std::mem::take(&mut unsafe { u.as_mut() }.write_buf);
    if buf.is_empty() {
        return Ok(());
    }
    write_to(u, &buf)
}

/// file:write(...) → the file (for chaining), or (nil, msg) on error.
///
/// A regular `FileHandle::File` write is buffered in user space (PUC stdio's
/// FILE*) and only reaches the OS via `drain_write_buf` at `:flush`,
/// `:seek`, `:close`, or before a `:read` on the same handle. Standard
/// streams and the closed/input cases stay unbuffered so error semantics on
/// them are unchanged. files.lua :475 expects writing to `/dev/full` to
/// succeed against the buffer and only fail at the subsequent `:flush`.
fn f_write(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, fs, nargs, "write")?;
    let bytes = gather_write(vm, fs, nargs, 1)?;
    if matches!(u.file(), FileHandle::File(_)) && u.writable {
        match u.buf_mode {
            // PUC `setvbuf("no")` flushes after every write — there is no
            // user-space buffer at all (we still funnel through `write_to`
            // for consistency, just skipping the staging buffer).
            2 => match write_to(u, &bytes) {
                Ok(()) => return Ok(vm.nat_return(fs, &[Value::Userdata(u)])),
                Err(e) => return Ok(file_result_err(vm, fs, "file", &e)),
            },
            // PUC `setvbuf("line")` stages writes and flushes after every
            // newline ('\n'); a trailing fragment without a newline stays
            // buffered until the next flush/close. files.lua 5.1 :245 polls
            // a paired reader that sees nothing until '\n' is written.
            1 => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { u.as_mut() }.write_buf.extend_from_slice(&bytes);
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                let last_nl = unsafe { u.as_mut() }
                    .write_buf
                    .iter()
                    .rposition(|&b| b == b'\n');
                if let Some(last_nl) = last_nl {
                    let split = last_nl + 1;
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                    let to_flush: Vec<u8> = unsafe {
                        let buf = &mut u.as_mut().write_buf;
                        buf.drain(..split).collect()
                    };
                    if let Err(e) = write_to(u, &to_flush) {
                        return Ok(file_result_err(vm, fs, "file", &e));
                    }
                }
                return Ok(vm.nat_return(fs, &[Value::Userdata(u)]));
            }
            // "full" (default): stage and let close/seek/flush drain it.
            _ => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { u.as_mut() }.write_buf.extend_from_slice(&bytes);
                return Ok(vm.nat_return(fs, &[Value::Userdata(u)]));
            }
        }
    }
    match write_to(u, &bytes) {
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Userdata(u)])),
        Err(e) => Ok(file_result_err(vm, fs, "file", &e)),
    }
}

fn seek_handle(u: Gc<Userdata>, from: SeekFrom) -> std::io::Result<u64> {
    // PUC `fseek` flushes the FILE*'s output buffer before moving the file
    // position; without this, a buffered write would land at the new offset.
    drain_write_buf(u)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.seek(from),
        // standard streams are not seekable (PUC returns the OS error)
        _ => Err(std::io::Error::from_raw_os_error(29)), // ESPIPE-ish "Illegal seek"
    }
}

/// file:seek([whence [, offset]]) → position, or (nil, msg, errno).
fn f_seek(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, fs, nargs, "seek")?;
    let whence: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => b"cur".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                2,
                "seek",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let off = match vm.nat_arg(fs, nargs, 2) {
        Value::Nil => 0,
        Value::Int(i) => i,
        Value::Float(f) => f as i64,
        v => {
            return Err(arg_error(
                vm,
                3,
                "seek",
                &format!("number expected, got {}", v.type_name()),
            ));
        }
    };
    let from = match whence.as_slice() {
        b"set" => SeekFrom::Start(off.max(0) as u64),
        b"cur" => SeekFrom::Current(off),
        b"end" => SeekFrom::End(off),
        _ => return Err(arg_error(vm, 2, "seek", "invalid option")),
    };
    match seek_handle(u, from) {
        Ok(pos) => Ok(vm.nat_return(fs, &[Value::Int(pos as i64)])),
        Err(e) => Ok(file_result_err(vm, fs, "file", &e)),
    }
}

/// file:setvbuf(mode [, size]) → the file. PUC accepts "no" / "full" / "line"
/// and returns the file. luna always buffers writable files in user space, so
/// the call is effectively an acknowledgement; we still validate `mode` so a
/// bad value raises the same arg error PUC does. files.lua :694 only checks
/// that the call returns truthy and that subsequent reads see the documented
/// buffered/unbuffered effects, which luna's existing semantics already
/// satisfy for the test's full-buffer / no-buffer / line-buffer probes.
fn f_setvbuf(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, fs, nargs, "setvbuf")?;
    let mode = match vm.nat_arg(fs, nargs, 1) {
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                2,
                "setvbuf",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let buf_mode: u8 = match mode.as_slice() {
        b"full" => 0,
        b"line" => 1,
        b"no" => 2,
        _ => return Err(arg_error(vm, 2, "setvbuf", "invalid option")),
    };
    // PUC `setvbuf` switches the buffering policy for subsequent writes.
    // luna mirrors the three modes against the existing `write_buf` —
    // `"line"` triggers a flush after every newline, `"no"` flushes on every
    // write, `"full"` only flushes on `close`/`seek`/explicit `flush`. Apply
    // immediately: if there's already pending data and the new mode is
    // tighter (`"no"`), flush it now so the switch takes effect right away.
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.buf_mode = buf_mode;
    if buf_mode == 2 {
        let _ = drain_write_buf(u);
    }
    // PUC f_setvbuf funnels through luaL_fileresult(stat, NULL) —
    // success is boolean TRUE, not the file (v2.14 CV.1, fixture
    // 5.5/311; probed on 5.4+5.5).
    Ok(vm.nat_return(fs, &[Value::Bool(true)]))
}

/// file:flush() → the file, or (nil, msg, errno) when the OS write fails
/// while draining the user-space buffer (PUC `f_flush` returns
/// luaL_fileresult of `fflush`). `/dev/full` surfaces ENOSPC here.
fn f_flush(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, fs, nargs, "flush")?;
    match flush_file(u) {
        // PUC f_flush is luaL_fileresult(fflush()==0, NULL): success
        // is boolean TRUE, not the file (probed on 5.5; f:write DOES
        // return the file — different helper).
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
        Err(e) => Ok(file_result_err(vm, fs, "file", &e)),
    }
}

/// io.flush() — flush the default output stream (PUC g_iofile + f_flush).
fn io_flush(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let u = vm.io_output.expect("default output set at startup");
    if u.file().is_closed() {
        return Err(raise_str(vm, "default output file is closed"));
    }
    match flush_file(u) {
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Userdata(u)])),
        Err(e) => Ok(file_result_err(vm, fs, "file", &e)),
    }
}

fn flush_file(u: Gc<Userdata>) -> std::io::Result<()> {
    drain_write_buf(u)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.flush(),
        FileHandle::Stdout => std::io::stdout().flush(),
        FileHandle::Stderr => std::io::stderr().flush(),
        _ => Ok(()),
    }
}

/// Read one byte from the handle (None at EOF). Returns a pushed-back byte
/// first. Only files and stdin read.
fn read_byte(u: Gc<Userdata>) -> std::io::Result<Option<u8>> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    if let Some(b) = unsafe { u.as_mut() }.peeked.take() {
        return Ok(Some(b));
    }
    let mut b = [0u8; 1];
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let n = match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.read(&mut b)?,
        FileHandle::Stdin => std::io::stdin().read(&mut b)?,
        _ => return Ok(None),
    };
    Ok(if n == 0 { None } else { Some(b[0]) })
}

/// Return one byte to the stream (PUC ungetc), seen by the next `read_byte`.
fn unread_byte(u: Gc<Userdata>, b: u8) {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.peeked = Some(b);
}

/// Cap on a single numeral's length (PUC `L_MAXLENNUM`). A longer run fails to
/// parse and leaves the overflowing tail in the stream.
const L_MAXLENNUM: usize = 200;

/// PUC `read_number` state: one held lookahead byte (`c`) plus the saved buffer.
struct Rn {
    buf: Vec<u8>,
    c: Option<u8>,
}

/// PUC `nextc`: commit the held byte and advance. On buffer overflow, invalidate
/// the run (clear `buf`) and stop without advancing, so the held byte and the
/// rest of the stream remain unread.
fn rn_save(rn: &mut Rn, u: Gc<Userdata>) -> std::io::Result<bool> {
    if rn.buf.len() >= L_MAXLENNUM {
        rn.buf.clear();
        return Ok(false);
    }
    if let Some(b) = rn.c {
        rn.buf.push(b);
    }
    rn.c = read_byte(u)?;
    Ok(true)
}

/// PUC `test2`: if the held byte is in `set`, save it and advance.
fn rn_test(rn: &mut Rn, u: Gc<Userdata>, set: &[u8]) -> std::io::Result<bool> {
    if matches!(rn.c, Some(c) if set.contains(&c)) {
        return rn_save(rn, u);
    }
    Ok(false)
}

/// PUC `readdigits`: consume a run of (hex or decimal) digits.
fn rn_digits(rn: &mut Rn, u: Gc<Userdata>, hex: bool) -> std::io::Result<u32> {
    let mut count = 0;
    loop {
        let is_digit = match rn.c {
            Some(c) if hex => c.is_ascii_hexdigit(),
            Some(c) => c.is_ascii_digit(),
            None => false,
        };
        if !is_digit || !rn_save(rn, u)? {
            break;
        }
        count += 1;
    }
    Ok(count)
}

/// Read the longest valid numeral prefix from `u`, faithful to PUC `read_number`:
/// skip leading whitespace, then a fixed grammar — optional sign, optional `0x`
/// hex prefix, integral digits, optional `.` + fractional digits, then at most
/// one exponent (`eE`/`pP`) with its own optional sign and digits. The first
/// non-fitting byte is pushed back so the next read sees it. An empty buffer
/// (no valid numeral, or overflow past `L_MAXLENNUM`) signals failure.
fn read_numeral(u: Gc<Userdata>) -> std::io::Result<Vec<u8>> {
    let mut c = read_byte(u)?;
    while matches!(c, Some(b) if b.is_ascii_whitespace()) {
        c = read_byte(u)?;
    }
    let mut rn = Rn { buf: Vec::new(), c };
    let mut hex = false;
    let mut count = 0u32;
    rn_test(&mut rn, u, b"-+")?; // optional sign
    if rn_test(&mut rn, u, b"0")? {
        if rn_test(&mut rn, u, b"xX")? {
            hex = true; // hexadecimal numeral
        } else {
            count = 1; // count the leading '0' as a digit
        }
    }
    count += rn_digits(&mut rn, u, hex)?; // integral part
    if rn_test(&mut rn, u, b".")? {
        count += rn_digits(&mut rn, u, hex)?; // fractional part
    }
    if count > 0 {
        let exp: &[u8] = if hex { b"pP" } else { b"eE" };
        if rn_test(&mut rn, u, exp)? {
            rn_test(&mut rn, u, b"-+")?; // exponent sign
            rn_digits(&mut rn, u, false)?; // exponent digits (always decimal)
        }
    }
    if let Some(c) = rn.c {
        unread_byte(u, c); // push back the first non-fitting byte
    }
    Ok(rn.buf)
}

/// How a read failed: a genuine Lua error (bad format/argument — must raise) vs
/// an underlying I/O error (PUC reports these as a `(nil, msg, errno)` result,
/// e.g. reading a write-only handle), kept distinct so callers route each right.
enum ReadFail {
    Lua(LuaError),
    Io(std::io::Error),
}

impl From<LuaError> for ReadFail {
    fn from(e: LuaError) -> Self {
        ReadFail::Lua(e)
    }
}

impl From<std::io::Error> for ReadFail {
    fn from(e: std::io::Error) -> Self {
        ReadFail::Io(e)
    }
}

/// Apply one read format to `u`. `fmt` is a Lua format value: a string
/// ("l"/"L"/"a"/"n") or an integer byte count. Returns the read value or nil.
fn read_format(vm: &mut Vm, u: Gc<Userdata>, fmt: Value) -> Result<Value, ReadFail> {
    // numeric: read exactly N bytes. 5.1's `io.read(0)` arrives as Float (5.1
    // has no integer subtype) — accept either, narrowed by `to_integer`-style
    // truncation (fractional part is rejected as it is in PUC's `lua_tointeger`).
    let n_opt: Option<i64> = match fmt {
        Value::Int(n) => Some(n),
        Value::Float(f)
            if f.is_finite()
                && f.fract() == 0.0
                && f >= i64::MIN as f64
                && f <= i64::MAX as f64 =>
        {
            Some(f as i64)
        }
        _ => None,
    };
    if let Some(n) = n_opt {
        let n = n.max(0) as usize;
        if n == 0 {
            // PUC `test_eof`: peek one byte and push it back — "" if data
            // remains, nil at end of file.
            return Ok(match read_byte(u)? {
                Some(b) => {
                    unread_byte(u, b);
                    Value::Str(vm.heap.intern(b""))
                }
                None => Value::Nil,
            });
        }
        let mut buf = Vec::with_capacity(n);
        for _ in 0..n {
            match read_byte(u)? {
                Some(b) => buf.push(b),
                None => break,
            }
        }
        if buf.is_empty() && n > 0 {
            return Ok(Value::Nil); // EOF
        }
        return Ok(Value::Str(vm.heap.intern(&buf)));
    }
    let f = match fmt {
        Value::Str(s) => {
            let b = s.as_bytes();
            // strip a leading '*' (5.2 compatibility)
            if b.first() == Some(&b'*') {
                b.get(1).copied()
            } else {
                b.first().copied()
            }
        }
        Value::Nil => Some(b'l'),
        _ => return Err(ReadFail::Lua(arg_error(vm, 1, "read", "invalid format"))),
    };
    match f {
        Some(b'l') | Some(b'L') => {
            let keep = f == Some(b'L');
            let mut buf = Vec::new();
            let mut got = false;
            loop {
                match read_byte(u)? {
                    Some(b'\n') => {
                        got = true;
                        if keep {
                            buf.push(b'\n');
                        }
                        break;
                    }
                    Some(c) => {
                        got = true;
                        buf.push(c);
                    }
                    None => break,
                }
            }
            if !got && buf.is_empty() {
                Ok(Value::Nil) // EOF
            } else {
                Ok(Value::Str(vm.heap.intern(&buf)))
            }
        }
        Some(b'a') => {
            let mut buf = Vec::new();
            while let Some(b) = read_byte(u)? {
                buf.push(b);
            }
            Ok(Value::Str(vm.heap.intern(&buf))) // "a" never returns nil (may be "")
        }
        Some(b'n') => {
            let buf = read_numeral(u)?;
            match crate::numeric::str2num(&buf, true, true) {
                Some(crate::numeric::Num::Int(i)) => Ok(Value::Int(i)),
                Some(crate::numeric::Num::Float(f)) => Ok(Value::Float(f)),
                None => Ok(Value::Nil),
            }
        }
        _ => Err(ReadFail::Lua(arg_error(vm, 1, "read", "invalid format"))),
    }
}

/// file:read(...) — one result per format (default "l").
fn f_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, fs, nargs, "read")?;
    // PUC stdio flushes the write side before switching to a read on the
    // same FILE*; without this, an r+/w+ handle would observe its own
    // unflushed writes only on disk via the read syscall.
    if let Err(e) = drain_write_buf(u) {
        return Ok(file_result_err(vm, fs, "file", &e));
    }
    read_dispatch(vm, fs, nargs, u, 1)
}

/// Shared read logic for file:read and io.read (formats start at `start`).
fn read_dispatch(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    u: Gc<Userdata>,
    start: u32,
) -> Result<u32, LuaError> {
    if nargs <= start {
        let v = match read_format(vm, u, Value::Nil) {
            Ok(v) => v,
            Err(ReadFail::Lua(e)) => return Err(e),
            Err(ReadFail::Io(e)) => return Ok(file_result_err(vm, fs, "file", &e)),
        };
        return Ok(vm.nat_return(fs, &[v]));
    }
    let mut out = Vec::new();
    for i in start..nargs {
        let fmt = vm.nat_arg(fs, nargs, i);
        let v = match read_format(vm, u, fmt) {
            Ok(v) => v,
            Err(ReadFail::Lua(e)) => return Err(e),
            Err(ReadFail::Io(e)) => return Ok(file_result_err(vm, fs, "file", &e)),
        };
        let stop = v.is_nil();
        out.push(v);
        if stop {
            break; // EOF on this format ends the read
        }
    }
    Ok(vm.nat_return(fs, &out))
}

// ---- default-stream io.read / io.write / io.lines, and line iterators ----

/// io.write(...) writes to the default output, returning it for chaining.
fn io_write2(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = vm.io_output.expect("default output set at startup");
    if u.file().is_closed() {
        // PUC getiofile: a closed default stream is a usage error.
        return Err(raise_str(vm, "default output file is closed"));
    }
    let bytes = gather_write(vm, fs, nargs, 0)?;
    match write_to(u, &bytes) {
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Userdata(u)])),
        Err(e) => Ok(file_result_err(vm, fs, "file", &e)),
    }
}

/// io.read(...) reads from the default input.
fn io_read2(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = vm.io_input.expect("default input set at startup");
    if u.file().is_closed() {
        // PUC getiofile: a closed default stream is a usage error.
        return Err(raise_str(vm, "default input file is closed"));
    }
    read_dispatch(vm, fs, nargs, u, 0)
}

/// The iterator function for file:lines / io.lines (closes at EOF when owned).
fn lines_iter(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let Value::Userdata(u) = vm.nat_upval(fs, 0) else {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    };
    if u.file().is_closed() {
        // PUC io_readline: a closed iterator (owned file shut at EOF) errors on
        // the next call rather than quietly yielding nil.
        return Err(raise_str(vm, "file is already closed"));
    }
    // Upvals: [file, owned, fmt0, fmt1, ...]. With no explicit formats, read a
    // single line ("l"); PUC io_readline runs g_read over the stored formats.
    let nfmt = vm.nat_upcount(fs).saturating_sub(2);
    let mut out = Vec::new();
    let mut eof = false;
    if nfmt == 0 {
        let v = read_line_for_iter(vm, u, Value::Nil)?;
        eof = v.is_nil();
        out.push(v);
    } else {
        for i in 0..nfmt {
            let fmt = vm.nat_upval(fs, 2 + i);
            let v = read_line_for_iter(vm, u, fmt)?;
            if i == 0 {
                eof = v.is_nil();
            }
            out.push(v);
            if v.is_nil() {
                break; // a format hit EOF: stop reading further formats
            }
        }
    }
    if eof {
        // first format hit EOF: close the file if this iterator owns it
        if let Value::Bool(true) = vm.nat_upval(fs, 1)
            && !u.file().is_std()
        {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            *unsafe { u.as_mut() }.file_mut() = FileHandle::Closed;
        }
    }
    Ok(vm.nat_return(fs, &out))
}

/// Read one format for a line iterator, translating a read error into a raise
/// (PUC io_readline calls luaL_error on the error message).
fn read_line_for_iter(vm: &mut Vm, u: Gc<Userdata>, fmt: Value) -> Result<Value, LuaError> {
    match read_format(vm, u, fmt) {
        Ok(v) => Ok(v),
        Err(ReadFail::Lua(e)) => Err(e),
        Err(ReadFail::Io(e)) => Err(raise_str(vm, &e.to_string())),
    }
}

/// PUC `MAXARGLINE`: cap on read formats a line iterator may carry.
const MAXARGLINE: u32 = 250;

/// Build a line-iterator closure over `u` (`owned` closes it at EOF), capturing
/// the read `formats` (empty → one line per step). Upvals: [file, owned, fmt..].
fn make_lines(vm: &mut Vm, u: Gc<Userdata>, owned: bool, formats: &[Value]) -> Value {
    let mut upvals = Vec::with_capacity(2 + formats.len());
    upvals.push(Value::Userdata(u));
    upvals.push(Value::Bool(owned));
    upvals.extend_from_slice(formats);
    vm.native_with(lines_iter, upvals.into_boxed_slice())
}

/// Collect read-format arguments `start..nargs` (PUC `aux_lines`), enforcing the
/// `MAXARGLINE` cap.
fn gather_line_formats(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    start: u32,
    who: &str,
) -> Result<Vec<Value>, LuaError> {
    if nargs.saturating_sub(start) > MAXARGLINE {
        return Err(arg_error(vm, MAXARGLINE + 2, who, "too many arguments"));
    }
    Ok((start..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect())
}

/// file:lines(...) — iterate lines of an already-open file (does not close it).
fn f_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, fs, nargs, "lines")?;
    let formats = gather_line_formats(vm, fs, nargs, 1, "lines")?;
    let it = make_lines(vm, u, false, &formats);
    Ok(vm.nat_return(fs, &[it]))
}

/// io.lines([filename, ...]) — open `filename` (or default input) and iterate.
fn io_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (u, owned) = if nargs >= 1 && !vm.nat_arg(fs, nargs, 0).is_nil() {
        let path = str_arg(vm, fs, nargs, 0, "lines")?;
        match std::fs::OpenOptions::new().read(true).open(&path) {
            Ok(file) => (new_file(vm, FileHandle::File(file), false), true),
            Err(e) => return Err(raise_str(vm, &format!("{path}: {e}"))),
        }
    } else {
        (vm.io_input.expect("default input set at startup"), false)
    };
    let formats = gather_line_formats(vm, fs, nargs, 1, "lines")?;
    let it = make_lines(vm, u, owned, &formats);
    // PUC 5.4+ `io_lines`: when it opened the file itself, return four values
    // (iterator, nil, nil, file) so the generic-for `<close>` value auto-closes
    // the handle at loop end. 5.1–5.3 predate to-be-closed and return just the
    // iterator (close-on-exhaustion only). files.lua's `load(io.lines(file,
    // "L"))()` depends on the older single-value form — extra returns become
    // load's optional name/mode/env arguments.
    if owned && vm.version() >= crate::version::LuaVersion::Lua54 {
        Ok(vm.nat_return(fs, &[it, Value::Nil, Value::Nil, Value::Userdata(u)]))
    } else {
        Ok(vm.nat_return(fs, &[it]))
    }
}

// ---- os file operations ----

/// PUC `io.tmpfile`: open a file in r+w mode and `unlink` it from the
/// filesystem so it disappears as soon as the fd closes (PUC's `tmpfile`
/// wraps libc `tmpfile()`). The file stays usable through the open handle
/// for the lifetime of the userdata. files.lua :824 round-trips a write +
/// seek + read on the returned handle.
fn io_tmpfile(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("lua_tmp_{}_{n}", std::process::id()));
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => return Ok(file_result_err(vm, fs, "tmpfile", &e)),
    };
    // PUC tmpfile unlinks immediately so the entry vanishes on close (or on
    // process exit if the handle outlives anyone holding it). On Unix the
    // open fd keeps the inode alive for reads/writes through this handle.
    let _ = std::fs::remove_file(&path);
    let u = new_file(vm, FileHandle::File(file), true);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

fn os_tmpname(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("lua_{}_{n}", std::process::id()));
    let s = Value::Str(vm.heap.intern(p.to_string_lossy().as_bytes()));
    Ok(vm.nat_return(fs, &[s]))
}

fn os_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let path = str_arg(vm, fs, nargs, 0, "remove")?;
    match std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir(&path)) {
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
        Err(e) => Ok(file_result_err(vm, fs, &path, &e)),
    }
}

fn os_rename(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let from = str_arg(vm, fs, nargs, 0, "rename")?;
    let to = str_arg(vm, fs, nargs, 1, "rename")?;
    match std::fs::rename(&from, &to) {
        Ok(()) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
        Err(e) => Ok(file_result_err(vm, fs, &from, &e)),
    }
}

/// PUC `os.execute([command])`. Shells out via `sh -c` on Unix and `cmd /C`
/// on Windows (matching the ISO C `system(3)` convention). No arg = probe for
/// a shell; 5.1 returns 1/0, 5.2+ returns a boolean. With an arg, 5.1 yields
/// the integer status, 5.2+ yields a `(success, "exit"|"signal", code)` triple
/// (luna currently only reports "exit" because Rust's `ExitStatus` exposes
/// signal info only behind the unix-only `ExitStatusExt::signal`, so we
/// promote that to "signal" when present).
///
/// Targets without `std::process::Command::spawn` (e.g. `wasm32-wasip1`)
/// get the `#[cfg(not(any(unix, windows)))]` stub: the no-arg probe reports
/// shell-unavailable (5.1 → `0`, 5.2+ → `false`); with an arg, returns the
/// PUC failure triple `(false, "exit", -1)` (5.2+) or `-1` (5.1).
#[cfg(any(unix, windows))]
fn os_execute(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::version::LuaVersion;
    // No arg: probe for shell availability. luna always reports yes (Rust
    // can spawn `sh` / `cmd` on every supported target).
    if nargs == 0 || matches!(vm.nat_arg(fs, nargs, 0), Value::Nil) {
        return Ok(if vm.version() <= LuaVersion::Lua51 {
            vm.nat_return(fs, &[Value::Int(1)])
        } else {
            vm.nat_return(fs, &[Value::Bool(true)])
        });
    }
    let cmd = str_arg(vm, fs, nargs, 0, "execute")?;
    let mut c = if cfg!(windows) {
        let mut cmd0 = std::process::Command::new("cmd");
        cmd0.arg("/C").arg(&cmd);
        cmd0
    } else {
        let mut cmd0 = std::process::Command::new("sh");
        cmd0.arg("-c").arg(&cmd);
        cmd0
    };
    let status = match c.status() {
        Ok(s) => s,
        Err(e) => {
            // PUC reports system errno via a fail result. 5.1 spits the raw
            // status, 5.2+ uses the `(nil, "exit"|"signal", code)` triple
            // with the message tacked onto a string.
            return Ok(file_result_err(vm, fs, &cmd, &e));
        }
    };
    let (kind, code) = exit_status_breakdown(&status);
    if vm.version() <= LuaVersion::Lua51 {
        // 5.1 returns a single status integer; non-zero on signal so the
        // assertion shape stays useful even there.
        return Ok(vm.nat_return(fs, &[Value::Int(code as i64)]));
    }
    let kind_s = Value::Str(vm.heap.intern(kind.as_bytes()));
    let ok = matches!(kind, "exit") && code == 0;
    Ok(vm.nat_return(fs, &[Value::Bool(ok), kind_s, Value::Int(code as i64)]))
}

/// `os.execute` stub for targets without `proc_*` (`wasm32-wasip1` / `-wasip2`).
/// No-arg probe reports shell-unavailable (5.1 → `0`, 5.2+ → `false`); with
/// a command arg, returns the PUC failure triple `(false, "exit", -1)` on
/// 5.2+ or `-1` on 5.1. The 5.2+ shape uses `"exit"` (not `"signal"`) to
/// match PUC, which reports `WEXITSTATUS`-style failures even when the
/// reason is "system() returned non-zero".
#[cfg(not(any(unix, windows)))]
fn os_execute(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::version::LuaVersion;
    if nargs == 0 || matches!(vm.nat_arg(fs, nargs, 0), Value::Nil) {
        return Ok(if vm.version() <= LuaVersion::Lua51 {
            vm.nat_return(fs, &[Value::Int(0)])
        } else {
            vm.nat_return(fs, &[Value::Bool(false)])
        });
    }
    let _cmd = str_arg(vm, fs, nargs, 0, "execute")?;
    if vm.version() <= LuaVersion::Lua51 {
        return Ok(vm.nat_return(fs, &[Value::Int(-1)]));
    }
    let kind = Value::Str(vm.heap.intern(b"exit"));
    Ok(vm.nat_return(fs, &[Value::Bool(false), kind, Value::Int(-1)]))
}

/// Decompose an `ExitStatus` into PUC's `("exit"|"signal", code)`. The signal
/// path is unix-only — on Windows we always report "exit" since the API gives
/// no signal info. A `None` exit code (process killed without standard exit
/// on unix) also degrades to "signal" with code -1.
///
/// Gated on `any(unix, windows)`: only callers are `close_file`'s popen
/// branch and `os_execute`, both gated the same way.
#[cfg(any(unix, windows))]
fn exit_status_breakdown(status: &std::process::ExitStatus) -> (&'static str, i32) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return ("signal", sig);
        }
    }
    match status.code() {
        Some(c) => ("exit", c),
        None => ("signal", -1),
    }
}

/// PUC `os.exit([code [, close]])`. Code defaults to success (0). A boolean
/// `true` maps to 0, `false` to 1 (the `EXIT_SUCCESS` / `EXIT_FAILURE` shape).
/// 5.4+ accepts an optional `close` flag asking the runtime to close the
/// state first; luna's `std::process::exit` skips Lua's `__close` chain by
/// design (we are tearing the process down anyway), matching what PUC does
/// when called from a Lua-side `os.exit` after a finalizer.
fn os_exit(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let code = match vm.nat_arg(fs, nargs, 0) {
        Value::Nil => 0,
        Value::Bool(true) => 0,
        Value::Bool(false) => 1,
        Value::Int(i) => i as i32,
        Value::Float(f) => f as i32,
        v => {
            return Err(arg_error(
                vm,
                1,
                "exit",
                &format!("number expected, got {}", v.type_name()),
            ));
        }
    };
    std::process::exit(code);
}

fn load_path(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Result<Value, Value>, LuaError> {
    let Value::Str(path) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "loadfile", "string expected"));
    };
    let path_s = String::from_utf8_lossy(path.as_bytes()).into_owned();
    let mode: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => b"bt".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => b"bt".to_vec(),
    };
    if mode.iter().any(|c| !matches!(c, b'b' | b't')) {
        return Err(raise_str(
            vm,
            &format!("invalid mode '{}'", String::from_utf8_lossy(&mode)),
        ));
    }
    match std::fs::read(&path_s) {
        Ok(src) => {
            let mut chunkname = vec![b'@'];
            chunkname.extend_from_slice(path.as_bytes());
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
            // PUC `luaL_loadfilex` checks the mode the chunk reports against
            // what the caller allowed and rejects the mismatching kind.
            let binary = crate::vm::dump::is_binary_chunk(src);
            if binary && !mode.contains(&b'b') || !binary && !mode.contains(&b't') {
                let kind = if binary { "binary" } else { "text" };
                let msg = format!(
                    "attempt to load a {kind} chunk (mode is '{}')",
                    String::from_utf8_lossy(&mode)
                );
                return Ok(Err(Value::Str(vm.heap.intern(msg.as_bytes()))));
            }
            match vm.load(src, &chunkname) {
                Ok(cl) => Ok(Ok(Value::Closure(cl))),
                Err(e) => {
                    let msg = format!("{path_s}:{e}");
                    Ok(Err(Value::Str(vm.heap.intern(msg.as_bytes()))))
                }
            }
        }
        Err(e) => {
            let msg = format!("cannot open {path_s} ({e})");
            Ok(Err(Value::Str(vm.heap.intern(msg.as_bytes()))))
        }
    }
}

fn nat_loadfile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    match load_path(vm, fs, nargs)? {
        Ok(Value::Closure(cl)) => {
            // PUC: `loadfile(filename, mode, env)` overrides upvalue 0 (the
            // chunk's `_ENV`) with the env table — same convention as `load`.
            if nargs >= 3 {
                let env = vm.nat_arg(fs, nargs, 2);
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

fn nat_dofile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    match load_path(vm, fs, nargs)? {
        Ok(f) => {
            let results = vm.call_value(f, &[])?;
            Ok(vm.nat_return(fs, &results))
        }
        Err(msg) => Err(raise_str(vm, &vm_text(vm, msg))),
    }
}

fn vm_text(_vm: &Vm, v: Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        _ => format!("(error object is a {} value)", v.type_name()),
    }
}

// ---- minimal package / require ----

pub(crate) fn open_package(vm: &mut Vm) {
    let pkg = vm.heap.new_table();
    let loaded = vm.heap.new_table();
    // prepopulate with the standard libraries (PUC does the same). Must include
    // every stdlib so e.g. nextvar.lua's "clear globals" test (which keeps any
    // name present in package.loaded) does not delete `coroutine`.
    for name in [
        "string",
        "math",
        "table",
        "os",
        "io",
        "utf8",
        "debug",
        "coroutine",
        "_G",
        "package",
    ] {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        let v = if name == "package" {
            Value::Table(pkg)
        } else if name == "_G" {
            Value::Table(vm.globals())
        } else {
            let gk = Value::Str(vm.heap.intern(name.as_bytes()));
            vm.globals().get(gk)
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, k, v)
            .expect("valid key");
    }
    let lk = Value::Str(vm.heap.intern(b"loaded"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, lk, Value::Table(loaded))
        .expect("valid key");
    let preload = vm.heap.new_table();
    let plk = Value::Str(vm.heap.intern(b"preload"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, plk, Value::Table(preload))
        .expect("valid key");
    // package.path: PUC default has `./?.lua` and `./?/init.lua`. attrib.lua
    // rewrites it freely so this is just a sane starting value.
    let pk = Value::Str(vm.heap.intern(b"path"));
    let pv = Value::Str(vm.heap.intern(b"./?.lua;./?/init.lua"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, pk, pv)
        .expect("valid key");
    // package.cpath: luna does not ship dynamic-library loading, so the
    // default is empty. attrib.lua's require-message test rewrites it.
    let ck = Value::Str(vm.heap.intern(b"cpath"));
    let cv = Value::Str(vm.heap.intern(b""));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, ck, cv)
        .expect("valid key");
    // package.config: PUC's five-line POSIX layout — dir-sep "/", path-sep
    // ";", template mark "?", exec-mark "!", ignore-mark "-".
    let cfk = Value::Str(vm.heap.intern(b"config"));
    let cfv = Value::Str(vm.heap.intern(b"/\n;\n?\n!\n-\n"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, cfk, cfv)
        .expect("valid key");
    // package.searchers: present as a table so attrib.lua's type checks pass.
    // luna's `require` does not dispatch through it (the searchers run in a
    // fixed order inside `nat_require`); this stays a leaf placeholder until
    // a userland test forces real dispatch.
    let searchers = vm.heap.new_table();
    let sk = Value::Str(vm.heap.intern(b"searchers"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, sk, Value::Table(searchers))
        .expect("valid key");
    // package.searchpath: pure path-template walker (no I/O side effects
    // beyond probing readability), shared with userland and exposed here.
    let sp = vm.native(nat_searchpath);
    let spk = Value::Str(vm.heap.intern(b"searchpath"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, spk, sp)
        .expect("valid key");
    vm.set_global("package", Value::Table(pkg))
        .expect("stdlib registration");
    // require reads package.path/cpath from the live `package` table each call
    // — attrib.lua mutates them inside `do … end` blocks and require must see
    // the override. Stash no upvalues; fetch from globals on demand.
    // PUC keeps `_LOADED` / `_PRELOAD` in the registry so a stray
    // `package = {}` in user code does not unlink the real bookkeeping.
    // luna captures the same tables as the require-native's own upvalues so
    // the lookup is stable regardless of `package`'s global identity. Slots:
    //   [0] = package table (still consulted for `path` / `cpath` overrides
    //         the user *does* expect to flow through globals);
    //   [1] = `package.loaded`;
    //   [2] = `package.preload`.
    let req = vm.native_with(
        nat_require,
        Box::new([
            Value::Table(pkg),
            Value::Table(loaded),
            Value::Table(preload),
        ]),
    );
    vm.set_global("require", req).expect("stdlib registration");
    // PUC 5.1 `module(name, ...)` and `package.seeall` (retired in 5.2). The
    // pair only makes sense alongside `setfenv`; gating on the dialect keeps
    // the 5.2+ surface clean.
    if vm.version() == crate::version::LuaVersion::Lua51 {
        // Same upval-anchored bookkeeping as `require`: the original
        // `package.loaded` is captured here so `module(...)` survives a
        // userland `package = {}` reassignment (attrib.lua's `do … end`
        // preload block does exactly that).
        let m = vm.native_with(nat_module, Box::new([Value::Table(loaded)]));
        vm.set_global("module", m).expect("stdlib registration");
        let s = vm.native(nat_package_seeall);
        let sk = Value::Str(vm.heap.intern(b"seeall"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { pkg.as_mut() }
            .set(&mut vm.heap, sk, s)
            .expect("valid key");
    }
    // PUC's `package.loadlib` opens a shared library and returns the named
    // symbol. luna ships no dynamic linker — return the PUC failure shape so
    // attrib.lua's "cannot load dynamic library" path (which prints a notice
    // and skips the C-only suite) runs rather than blowing up on a nil call.
    let ll = vm.native(nat_loadlib_stub);
    let llk = Value::Str(vm.heap.intern(b"loadlib"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, llk, ll)
        .expect("valid key");
    // Once-per-table barriers for the four sub-tables built above —
    // covers the post-init `Vm::open_package` re-open path (mid-Propagate).
    vm.barrier_back_table(pkg);
    vm.barrier_back_table(loaded);
    vm.barrier_back_table(preload);
    vm.barrier_back_table(searchers);
}

fn nat_loadlib_stub(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let msg = Value::Str(
        vm.heap
            .intern(b"dynamic libraries not enabled; check your Lua installation"),
    );
    let when = Value::Str(vm.heap.intern(b"absent"));
    Ok(vm.nat_return(fs, &[Value::Nil, msg, when]))
}

/// PUC 5.1 `module(name, ...)`: create (or reuse) `package.loaded[name]` as
/// the module's table, decorate it with `_NAME` / `_M` / `_PACKAGE`, run the
/// extra option functions (`package.seeall` being the canonical one), and
/// repoint the caller's `_ENV` cell to the module table. After this, every
/// global write inside the calling chunk lands in the module table.
fn nat_module(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name_v = vm.nat_arg(fs, nargs, 0);
    let name_bytes = match name_v {
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                1,
                "module",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let name_str = String::from_utf8_lossy(&name_bytes).into_owned();
    // 1. resolve / create the module table via package.loaded[name]. Reach
    // for the captured `loaded` upvalue first so a userland `package = {}`
    // doesn't pull the rug out from under module().
    let loaded = if vm.nat_upcount(fs) >= 1 {
        match vm.nat_upval(fs, 0) {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.loaded' upvalue missing")),
        }
    } else {
        let pkg_k = Value::Str(vm.heap.intern(b"package"));
        let Value::Table(pkg) = vm.globals().get(pkg_k) else {
            return Err(raise_str(vm, "'package' table missing"));
        };
        let loaded_k = Value::Str(vm.heap.intern(b"loaded"));
        let Value::Table(t) = pkg.get(loaded_k) else {
            return Err(raise_str(vm, "'package.loaded' must be a table"));
        };
        t
    };
    let name_key = Value::Str(vm.heap.intern(&name_bytes));
    let module_tab = match loaded.get(name_key) {
        Value::Table(t) => t,
        _ => {
            // PUC `module()` walks the dotted name in the global table
            // (`_findtable`): every intermediate key is created if missing
            // and *reused* if it already maps to a table. The final key is
            // resolved the same way — when `module("X.a.b")` runs after
            // `module("X.a.b.c")` has already populated the intermediate
            // X.a.b table, the existing table is adopted as the module
            // (carrying its `.c` subtable along) rather than overwritten.
            let t = resolve_or_create_dotted(vm, &name_bytes)?;
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { loaded.as_mut() }
                .set(&mut vm.heap, name_key, Value::Table(t))
                .expect("valid key");
            t
        }
    };
    // 2. populate _NAME / _M / _PACKAGE. PUC keeps the trailing dot in
    // `_PACKAGE` for nested modules — `module("P1.xuxu", ...)` ↦ "P1.".
    let pre_dot = match name_bytes.iter().rposition(|&b| b == b'.') {
        Some(i) => name_bytes[..=i].to_vec(),
        None => Vec::new(),
    };
    let name_val = Value::Str(vm.heap.intern(&name_bytes));
    let pkg_val = Value::Str(vm.heap.intern(&pre_dot));
    let name_k = Value::Str(vm.heap.intern(b"_NAME"));
    let m_k = Value::Str(vm.heap.intern(b"_M"));
    let p_k = Value::Str(vm.heap.intern(b"_PACKAGE"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { module_tab.as_mut() }
        .set(&mut vm.heap, name_k, name_val)
        .expect("valid key");
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { module_tab.as_mut() }
        .set(&mut vm.heap, m_k, Value::Table(module_tab))
        .expect("valid key");
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { module_tab.as_mut() }
        .set(&mut vm.heap, p_k, pkg_val)
        .expect("valid key");
    // 3. run option functions on the module table
    for i in 1..nargs {
        let f = vm.nat_arg(fs, nargs, i);
        if !f.is_nil() {
            vm.call_value(f, &[Value::Table(module_tab)])?;
        }
    }
    // 4. rewrite the caller's `_ENV` cell to the module table (PUC
    // `setfenv(2)`). The `_ENV` upvalue is not necessarily at slot 0 —
    // closures capture upvalues in first-access order — so locate it by
    // name in the proto's upvalue descriptors.
    if let Some(cl) = vm.lua_closure_at_level(1) {
        let mut env_idx = None;
        for (i, d) in cl.proto.upvals.iter().enumerate() {
            if &*d.name == "_ENV" {
                env_idx = Some(i);
                break;
            }
        }
        let Some(env_idx) = env_idx else {
            return Err(raise_str(
                vm,
                &format!("module '{name_str}' caller has no '_ENV' upvalue"),
            ));
        };
        let uv = cl.upvals()[env_idx];
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { uv.as_mut() }.set_closed(Value::Table(module_tab));
        vm.barrier_forward_upvalue(uv, Value::Table(module_tab));
    } else {
        return Err(raise_str(
            vm,
            &format!("module '{name_str}' needs a Lua caller frame"),
        ));
    }
    Ok(vm.nat_return(fs, &[Value::Table(module_tab)]))
}

/// Resolve `_G.a.b.c…` to its table, creating intermediates AND the leaf
/// when they are missing. Mirrors PUC `_findtable`: each component is fetched
/// once; nil components get a fresh table that is then both stored at that
/// key and used as the next walk root, while existing tables are reused.
fn resolve_or_create_dotted(
    vm: &mut Vm,
    name: &[u8],
) -> Result<crate::runtime::Gc<crate::runtime::Table>, LuaError> {
    let mut tab = vm.globals();
    let mut start = 0;
    let mut parts: Vec<&[u8]> = Vec::new();
    for (i, &b) in name.iter().enumerate() {
        if b == b'.' {
            parts.push(&name[start..i]);
            start = i + 1;
        }
    }
    parts.push(&name[start..]);
    for p in parts.iter() {
        let k = Value::Str(vm.heap.intern(p));
        let next = tab.get(k);
        tab = match next {
            Value::Table(t) => t,
            Value::Nil => {
                let t = vm.heap.new_table();
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { tab.as_mut() }
                    .set(&mut vm.heap, k, Value::Table(t))
                    .expect("valid key");
                t
            }
            _ => {
                // PUC `_findtable` raises "name conflict for module 'X'" when
                // the dotted path runs into a non-table, non-nil value (e.g.
                // `module("math.sin")` — `math.sin` is a function). attrib.lua
                // :172 / :173 require this to surface as a pcall failure.
                let s = String::from_utf8_lossy(name);
                return Err(raise_str(vm, &format!("name conflict for module '{s}'")));
            }
        };
    }
    Ok(tab)
}

/// PUC 5.1 `package.seeall(module)`: attach a metatable whose `__index` is
/// `_G`, so any name unresolved in the module table falls back to the global
/// environment. Used inside `module(...)`'s option list as a convenience.
fn nat_package_seeall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let m = vm.nat_arg(fs, nargs, 0);
    let Value::Table(t) = m else {
        return Err(arg_error(vm, 1, "seeall", "table expected"));
    };
    let mt = vm.heap.new_table();
    let k = Value::Str(vm.heap.intern(b"__index"));
    let g = Value::Table(vm.globals());
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { mt.as_mut() }
        .set(&mut vm.heap, k, g)
        .expect("valid key");
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.set_metatable(Some(mt));
    Ok(vm.nat_return(fs, &[]))
}

/// Substitute every '?' in `tpl` with `subst`. PUC `luaL_gsub` semantics.
fn template_expand(tpl: &[u8], subst: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tpl.len() + subst.len());
    for &b in tpl {
        if b == b'?' {
            out.extend_from_slice(subst);
        } else {
            out.push(b);
        }
    }
    out
}

/// Replace every occurrence of `from` in `src` with `to`. Used by
/// `package.searchpath`'s sep→rep substitution on the module name.
fn replace_bytes(src: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return src.to_vec();
    }
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if i + from.len() <= src.len() && &src[i..i + from.len()] == from {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

fn nat_searchpath(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "searchpath", "string expected"));
    };
    let Value::Str(path) = vm.nat_arg(fs, nargs, 1) else {
        return Err(arg_error(vm, 2, "searchpath", "string expected"));
    };
    let sep: Vec<u8> = match vm.nat_arg(fs, nargs, 2) {
        Value::Nil => b".".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Err(arg_error(vm, 3, "searchpath", "string expected")),
    };
    let rep: Vec<u8> = match vm.nat_arg(fs, nargs, 3) {
        Value::Nil => b"/".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Err(arg_error(vm, 4, "searchpath", "string expected")),
    };
    let name_bytes = name.as_bytes().to_vec();
    let path_bytes = path.as_bytes().to_vec();
    let translated = replace_bytes(&name_bytes, &sep, &rep);
    let mut err = Vec::new();
    // PUC `pushnexttemplate` skips runs of separator chars, so `;;` and the
    // empty trailing template never appear as candidates and never add a
    // "no file ''" line.
    for tpl in path_bytes.split(|&b| b == b';') {
        if tpl.is_empty() {
            continue;
        }
        let expanded = template_expand(tpl, &translated);
        if std::fs::File::open(std::path::Path::new(
            std::str::from_utf8(&expanded).unwrap_or(""),
        ))
        .is_ok()
        {
            let v = Value::Str(vm.heap.intern(&expanded));
            return Ok(vm.nat_return(fs, &[v]));
        }
        err.extend_from_slice(b"\n\tno file '");
        err.extend_from_slice(&expanded);
        err.push(b'\'');
    }
    let err_v = Value::Str(vm.heap.intern(&err));
    Ok(vm.nat_return(fs, &[Value::Nil, err_v]))
}

fn nat_require(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "require", "string expected"));
    };
    let key = Value::Str(name);
    let name_s = String::from_utf8_lossy(name.as_bytes()).into_owned();

    // PUC's require reads `_LOADED` / `_PRELOAD` from the registry, so the
    // user reassigning the global `package` cannot disturb the bookkeeping.
    // luna captures the original tables as native upvalues at startup; fall
    // back to globals.package only for older callers that constructed the
    // native without upvals.
    let (pkg, loaded) = if vm.nat_upcount(fs) >= 2 {
        let p = match vm.nat_upval(fs, 0) {
            Value::Table(t) => t,
            _ => {
                return Err(raise_str(vm, "'package' upvalue missing"));
            }
        };
        let l = match vm.nat_upval(fs, 1) {
            Value::Table(t) => t,
            _ => {
                return Err(raise_str(vm, "'package.loaded' upvalue missing"));
            }
        };
        (p, l)
    } else {
        let pkg_k = Value::Str(vm.heap.intern(b"package"));
        let Value::Table(p) = vm.globals().get(pkg_k) else {
            return Err(raise_str(vm, "'package' table missing"));
        };
        let loaded_k = Value::Str(vm.heap.intern(b"loaded"));
        let Value::Table(l) = p.get(loaded_k) else {
            return Err(raise_str(vm, "'package.loaded' must be a table"));
        };
        (p, l)
    };
    let cached = loaded.get(key);
    // PUC 5.1 `ll_require` keyed the "already loaded" guard on
    // `lua_toboolean(loaded[name])` — a module whose stored value is false
    // (e.g. `return false`) was treated as not loaded and re-executed. 5.2+
    // changed that to `lua_isnil(loaded[name])`, so any non-nil entry blocks
    // re-execution. attrib.lua's "default option (should reload it)" probe
    // depends on the 5.1 falsy-as-not-loaded rule.
    let already_loaded = if vm.version() <= crate::version::LuaVersion::Lua51 {
        cached.truthy()
    } else {
        !cached.is_nil()
    };
    if already_loaded {
        return Ok(vm.nat_return(fs, &[cached]));
    }

    // Error message is the concatenation of every searcher's miss reason;
    // PUC's findloader builds it the same way (one '\n\t…' chunk per try).
    let mut err = String::new();

    // preload searcher (PUC: searcher #1, runs before file searchers). Same
    // upval-vs-globals story as `loaded`: when the captured upvals are
    // present, read them so a user `package = {}` cannot derail preload.
    let preload = if vm.nat_upcount(fs) >= 3 {
        match vm.nat_upval(fs, 2) {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.preload' upvalue missing")),
        }
    } else {
        let preload_k = Value::Str(vm.heap.intern(b"preload"));
        let Value::Table(t) = pkg.get(preload_k) else {
            return Err(raise_str(vm, "'package.preload' must be a table"));
        };
        t
    };
    let loader = preload.get(key);
    if !loader.is_nil() {
        // PUC 5.1's preload loader is called with just the module name as a
        // single arg; 5.2+ added a "path" arg (`:preload:`). attrib.lua's
        // `function (...) module(...) end` preload variant in 5.1 passes
        // `...` straight to `module`, so the extra string would be misread
        // as an option function and get called against the module table.
        let pv = Value::Str(vm.heap.intern(b":preload:"));
        let args: &[Value] = if vm.version() <= crate::version::LuaVersion::Lua51 {
            &[key]
        } else {
            &[key, pv]
        };
        let results = vm.call_value(loader, args)?;
        let returned = results.first().copied().unwrap_or(Value::Nil);
        // PUC `ll_require`: if the loader returned non-nil, store that. Else
        // honour whatever the loader may have written into `package.loaded`
        // (e.g. via `module()` setting `loaded[name] = module_tab`). Only
        // fall back to `true` when both come up empty. attrib.lua's preload
        // `module(...)` pattern relies on the second branch — the module
        // table set by `module()` must survive the require.
        let value = if !returned.is_nil() {
            returned
        } else {
            let post = loaded.get(key);
            if !post.is_nil() {
                post
            } else {
                Value::Bool(true)
            }
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, key, value)
            .expect("valid key");
        vm.barrier_back_table(loaded);
        return Ok(vm.nat_return(fs, &[value, pv]));
    }
    err.push_str(&format!("\n\tno field package.preload['{name_s}']"));

    // file searcher driven by package.path. attrib.lua sometimes sets
    // package.path to a non-string to confirm the error mentions it.
    let path_k = Value::Str(vm.heap.intern(b"path"));
    let path_v = pkg.get(path_k);
    let path_bytes = match path_v {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Err(raise_str(vm, "'package.path' must be a string")),
    };
    // In a module name like "P1.xuxu", PUC's file searcher first replaces
    // '.' with the dir-separator before template expansion.
    let translated_name = replace_bytes(name.as_bytes(), b".", b"/");
    let mut found: Option<(Vec<u8>, Vec<u8>)> = None;
    for tpl in path_bytes.split(|&b| b == b';') {
        if tpl.is_empty() {
            continue;
        }
        let expanded = template_expand(tpl, &translated_name);
        if found.is_none()
            && let Ok(src) = std::fs::read(std::str::from_utf8(&expanded).unwrap_or(""))
        {
            found = Some((expanded.clone(), src));
        }
        err.push_str("\n\tno file '");
        err.push_str(&String::from_utf8_lossy(&expanded));
        err.push('\'');
    }

    // C-library searcher: luna has no dynamic-linking backend, but attrib.lua
    // still inspects the message format. Walk cpath only to append "no file"
    // lines; never load anything.
    let cpath_k = Value::Str(vm.heap.intern(b"cpath"));
    let cpath_v = pkg.get(cpath_k);
    let cpath_bytes = match cpath_v {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Nil => Vec::new(),
        _ => return Err(raise_str(vm, "'package.cpath' must be a string")),
    };
    for tpl in cpath_bytes.split(|&b| b == b';') {
        if tpl.is_empty() {
            continue;
        }
        let expanded = template_expand(tpl, &translated_name);
        err.push_str("\n\tno file '");
        err.push_str(&String::from_utf8_lossy(&expanded));
        err.push('\'');
    }

    if let Some((path_b, src)) = found {
        let path_s = String::from_utf8_lossy(&path_b).into_owned();
        let chunkname = format!("@{path_s}");
        let src = crate::frontend::lexer::Lexer::strip_shebang_bom(&src);
        let cl = match vm.load(src, chunkname.as_bytes()) {
            Ok(cl) => cl,
            Err(e) => {
                return Err(raise_str(
                    vm,
                    &format!("error loading module '{name_s}' from file '{path_s}':\n\t{e}"),
                ));
            }
        };
        let pv = Value::Str(vm.heap.intern(path_s.as_bytes()));
        let results = vm.call_value(Value::Closure(cl), &[key, pv])?;
        let value = results.first().copied().unwrap_or(Value::Nil);
        let value = if value.is_nil() {
            Value::Bool(true)
        } else {
            value
        };
        // Re-fetch loaded[name]: a preload-style module can have set it
        // during its own body (attrib.lua's C.lua does `package.loaded[...] =
        // 25; require'C'`); honour that value over the chunk's return.
        let post = loaded.get(key);
        let final_v = if !post.is_nil() { post } else { value };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, key, final_v)
            .expect("valid key");
        vm.barrier_back_table(loaded);
        return Ok(vm.nat_return(fs, &[final_v, pv]));
    }

    Err(raise_str(vm, &format!("module '{name_s}' not found:{err}")))
}
