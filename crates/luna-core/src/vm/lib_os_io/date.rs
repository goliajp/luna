//! `os.date` and its `strftime` conversions.

use super::*;

pub(super) fn os_date(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
