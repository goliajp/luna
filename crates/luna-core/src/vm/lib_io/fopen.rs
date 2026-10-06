//! How `fopen` reads a mode and opens a file: by the rules of glibc, or of
//! the Universal CRT when the Vm opens files as PUC built with MSVC does;
//! on Windows through the `CreateFileW` call that library makes, so that a
//! failure has its error.

use super::msvc::TextMode;

/// What a mode asks of `fopen`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Spec {
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) append: bool,
    pub(crate) create: bool,
    pub(crate) truncate: bool,
    pub(crate) excl: bool,
    /// `+`: reading and writing (`_O_RDWR`)
    pub(crate) update: bool,
    /// `b`
    pub(crate) binary: bool,
    /// `D` (`_O_TEMPORARY`): the file goes when it is closed
    pub(crate) temporary: bool,
    /// `N` (`_O_NOINHERIT`)
    pub(crate) no_inherit: bool,
    /// the encoding a `ccs=` names
    pub(crate) ccs: Option<Ccs>,
}

/// The encodings of the Universal CRT's `ccs=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ccs {
    /// `UTF-8` (`_O_U8TEXT`)
    Utf8,
    /// `UTF-16LE` (`_O_U16TEXT`)
    Utf16le,
    /// `UNICODE` (`_O_WTEXT`)
    Unicode,
}

impl Spec {
    /// `"r"`
    pub(crate) const READ: Spec = Spec {
        read: true,
        write: false,
        append: false,
        create: false,
        truncate: false,
        excl: false,
        update: false,
        binary: false,
        temporary: false,
        no_inherit: false,
        ccs: None,
    };

    /// Whether the stream can be written.
    pub(crate) fn writable(&self) -> bool {
        self.write || self.update
    }

    /// The first letter alone, `r`, `w` or `a`.
    fn first(c: u8) -> Option<Spec> {
        let mut s = Spec::default();
        match c {
            b'r' => s.read = true,
            b'w' => (s.write, s.create, s.truncate) = (true, true, true),
            b'a' => (s.write, s.create, s.append) = (true, true, true),
            _ => return None,
        }
        Some(s)
    }
}

/// The mode as luna's own `fopen` takes it, the C library of a Unix
/// system: a first letter, then anywhere a `+` and an `x` (`O_EXCL`); other
/// letters are ignored. `None` is EINVAL.
pub(crate) fn libc_mode(mode: &[u8]) -> Option<Spec> {
    let mut s = Spec::first(*mode.first()?)?;
    let rest = &mode[1..];
    s.update = rest.contains(&b'+');
    s.binary = rest.contains(&b'b');
    s.excl = s.create && rest.contains(&b'x');
    Some(s)
}

/// `__acrt_stdio_parse_mode`: `None` when the C library calls the mode
/// invalid, which ends the process (see [`crt::invalid_parameter`]).
pub(crate) fn ucrt_mode(mode: &[u8]) -> Option<Spec> {
    let at = |i: usize| mode.get(i).copied().unwrap_or(0);
    let skip_spaces = |mut i: usize| {
        while at(i) == b' ' {
            i += 1;
        }
        i
    };
    let mut i = skip_spaces(0);
    let mut s = Spec::first(at(i))?;
    i += 1;
    // each of these may appear once; a second one ends the letters there,
    // and the mode is then invalid unless it ends
    let (mut plus, mut tb, mut commit, mut scan, mut short) = (false, false, false, false, false);
    let mut encoding = false;
    loop {
        let ok = match at(i) {
            0 => break,
            b'+' => {
                !std::mem::replace(&mut plus, true) && !s.update && {
                    s.update = true;
                    true
                }
            }
            c @ (b'b' | b't') => {
                !tb && {
                    tb = true;
                    s.binary = c == b'b';
                    true
                }
            }
            b'c' | b'n' => !std::mem::replace(&mut commit, true),
            b'S' | b'R' => !std::mem::replace(&mut scan, true),
            b'T' => !std::mem::replace(&mut short, true),
            b'D' => !std::mem::replace(&mut s.temporary, true),
            b'N' => {
                s.no_inherit = true;
                true
            }
            b'x' => {
                s.truncate && {
                    s.excl = true;
                    true
                }
            }
            b' ' => true,
            b',' => {
                encoding = true;
                i += 1;
                break;
            }
            _ => return None,
        };
        if !ok {
            break;
        }
        i += 1;
    }
    i = skip_spaces(i);
    if !encoding {
        return (at(i) == 0).then_some(s);
    }
    if !mode[i..].starts_with(b"ccs") {
        return None;
    }
    i = skip_spaces(i + 3);
    if at(i) != b'=' {
        return None;
    }
    i = skip_spaces(i + 1);
    let named = |name: &[u8]| {
        mode.get(i..i + name.len())
            .is_some_and(|m| m.eq_ignore_ascii_case(name))
    };
    let (ccs, len) = if named(b"UTF-8") {
        (Ccs::Utf8, 5)
    } else if named(b"UTF-16LE") {
        (Ccs::Utf16le, 8)
    } else if named(b"UNICODE") {
        (Ccs::Unicode, 7)
    } else {
        return None;
    };
    s.ccs = Some(ccs);
    i = skip_spaces(i + len);
    (at(i) == 0).then_some(s)
}

/// Open `name` as `spec` asks.
#[cfg(not(windows))]
pub(crate) fn os_open(name: &[u8], spec: &Spec) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    // an appending stream in a Unicode mode reads its byte order mark
    o.read(spec.read || spec.update || (spec.append && spec.ccs.is_some()));
    if spec.append {
        o.append(true);
    } else {
        o.write(spec.writable());
    }
    o.truncate(spec.truncate);
    if spec.excl {
        o.create_new(true);
    } else {
        o.create(spec.create);
    }
    o.open(super::os_path(name))
}

/// Open `name` as `spec` asks, with the `CreateFileW` call of the Universal
/// CRT's `_wsopen_nolock`. An appending stream gets append access, so that
/// the system writes at the end as the C library's `FAPPEND` does.
#[cfg(windows)]
pub(crate) fn os_open(name: &[u8], spec: &Spec) -> std::io::Result<std::fs::File> {
    use super::winfs::*;
    let write = if spec.append {
        APPEND_DATA
    } else {
        GENERIC_WRITE
    };
    let mut access = if spec.update {
        GENERIC_READ | write
    } else if spec.write && spec.ccs.is_some() && spec.append {
        // to read the BOM of an existing file first
        GENERIC_READ | write
    } else if spec.write {
        write
    } else {
        GENERIC_READ
    };
    let create = match (spec.create, spec.truncate, spec.excl) {
        (true, _, true) => CREATE_NEW,
        (true, true, false) => CREATE_ALWAYS,
        (true, false, false) => OPEN_ALWAYS,
        (false, true, _) => TRUNCATE_EXISTING,
        (false, false, _) => OPEN_EXISTING,
    };
    let mut share = FILE_SHARE_READ | FILE_SHARE_WRITE;
    let mut flags = FILE_ATTRIBUTE_NORMAL;
    if spec.temporary {
        flags |= FILE_FLAG_DELETE_ON_CLOSE;
        access |= DELETE;
        share |= FILE_SHARE_DELETE;
    }
    let path = wide(name);
    let open = |access| create_file(&path, access, share, !spec.no_inherit, create, flags);
    match open(access) {
        Err(_) if access & GENERIC_READ != 0 && spec.write && !spec.update => {
            open(access & !GENERIC_READ)
        }
        r => r,
    }
}

/// `configure_text_mode` of `_wsopen_nolock`, for a file opened in text
/// mode: the mode a `ccs=` selects, from the file's byte order mark when it
/// is read from an existing file, with the mark written when the file
/// starts empty. A UTF-16 big-endian mark is EINVAL.
pub(crate) fn text_mode(f: &mut std::fs::File, spec: &Spec) -> std::io::Result<TextMode> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let Some(ccs) = spec.ccs else {
        return Ok(TextMode::Ansi);
    };
    let mut mode = match ccs {
        Ccs::Utf8 => TextMode::Utf8,
        Ccs::Utf16le => TextMode::Utf16le,
        // UTF-16 only when the file is made anew for writing alone
        Ccs::Unicode if spec.write && spec.truncate && !spec.update => TextMode::Utf16le,
        Ccs::Unicode => TextMode::Ansi,
    };
    if !f.metadata()?.is_file() {
        return Ok(mode);
    }
    // an appending stream in a Unicode mode is opened for reading as well
    let reads = spec.read || spec.update || spec.append;
    let (check_bom, write_bom) = if !spec.writable() {
        (true, false)
    } else if spec.truncate || spec.excl {
        (false, true)
    } else if f.seek(SeekFrom::End(0))? != 0 {
        f.seek(SeekFrom::Start(0))?;
        (reads, false)
    } else {
        (false, true)
    };
    if check_bom {
        let mut bom = [0u8; 3];
        let n = f.read(&mut bom)?;
        if n == 3 && bom == [0xEF, 0xBB, 0xBF] {
            mode = TextMode::Utf8;
        } else if n >= 2 && bom[..2] == [0xFE, 0xFF] {
            return Err(super::posix_error(super::EINVAL));
        } else if n >= 2 && bom[..2] == [0xFF, 0xFE] {
            f.seek(SeekFrom::Start(2))?;
            mode = TextMode::Utf16le;
        } else {
            f.seek(SeekFrom::Start(0))?;
        }
    }
    if write_bom {
        match mode {
            TextMode::Utf16le => f.write_all(&[0xFF, 0xFE])?,
            TextMode::Utf8 => f.write_all(&[0xEF, 0xBB, 0xBF])?,
            TextMode::Ansi => {}
        }
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_as_the_ucrt_parses_them() {
        let ok =
            |m: &[u8]| ucrt_mode(m).unwrap_or_else(|| panic!("{:?}", String::from_utf8_lossy(m)));
        let r = ok(b"r");
        assert!(r.read && !r.writable() && !r.binary && r.ccs.is_none());
        let w = ok(b" w+b ");
        assert!(w.update && w.truncate && w.create && w.binary);
        let a = ok(b"at+cSTDN");
        assert!(a.append && a.update && a.temporary && a.no_inherit && !a.binary);
        assert!(ok(b"wx").excl);
        assert_eq!(ok(b"r, ccs=UTF-8").ccs, Some(Ccs::Utf8));
        assert_eq!(ok(b"w,ccs = utf-16le ").ccs, Some(Ccs::Utf16le));
        assert_eq!(ok(b"a+, ccs=unicode").ccs, Some(Ccs::Unicode));
        // what the library calls invalid
        for m in [
            &b""[..],
            b"x",
            b"rw",
            b"r++",
            b"rbt",
            b"rbb",
            b"rx",
            b"r?",
            b"r, ccs=KOI8",
            b"r, CCS=UTF-8",
            b"r ccs=UTF-8",
            b"r, ccs=UTF-8, ccs=UTF-8",
            b"r, ccs=UTF-8x",
        ] {
            assert_eq!(ucrt_mode(m), None, "{:?}", String::from_utf8_lossy(m));
        }
    }

    #[test]
    fn modes_parse_as_glibc_takes_them() {
        let s = libc_mode(b"rw+x").expect("valid");
        assert!(s.read && s.update && !s.excl);
        assert!(libc_mode(b"wx").expect("valid").excl);
        assert_eq!(libc_mode(b"q"), None);
        assert_eq!(libc_mode(b""), None);
    }
}
