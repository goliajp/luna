//! The Win32 calls the Universal CRT makes to open, remove and rename
//! files, made the same way so that a failure gives the same error, and
//! the conversions its narrow functions make between the bytes a program
//! hands them and the UTF-16 the system takes: through the ANSI code page
//! (`GetACP`), as `MultiByteToWideChar` and `WideCharToMultiByte` do it.

use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::FromRawHandle;

pub(super) const GENERIC_READ: u32 = 0x8000_0000;
pub(super) const GENERIC_WRITE: u32 = 0x4000_0000;
/// `FILE_GENERIC_WRITE` without `FILE_WRITE_DATA`: writes go to the end
pub(super) const APPEND_DATA: u32 = 0x0012_0116 & !0x2;
pub(super) const DELETE: u32 = 0x0001_0000;
pub(super) const FILE_SHARE_READ: u32 = 0x1;
pub(super) const FILE_SHARE_WRITE: u32 = 0x2;
pub(super) const FILE_SHARE_DELETE: u32 = 0x4;
pub(super) const CREATE_NEW: u32 = 1;
pub(super) const CREATE_ALWAYS: u32 = 2;
pub(super) const OPEN_EXISTING: u32 = 3;
pub(super) const OPEN_ALWAYS: u32 = 4;
pub(super) const TRUNCATE_EXISTING: u32 = 5;
pub(super) const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
pub(super) const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const MOVEFILE_COPY_ALLOWED: u32 = 0x2;
const INVALID_HANDLE_VALUE: isize = -1;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

const MB_PRECOMPOSED: u32 = 0x1;
const MB_ERR_INVALID_CHARS: u32 = 0x8;

#[link(name = "kernel32")]
// SAFETY: the declarations match `CreateFileW`, `DeleteFileW`, `MoveFileExW`, `GetACP`, `MultiByteToWideChar` and `WideCharToMultiByte` in kernel32
unsafe extern "system" {
    fn GetACP() -> u32;
    fn MultiByteToWideChar(
        cp: u32,
        flags: u32,
        s: *const u8,
        n: i32,
        out: *mut u16,
        cap: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        cp: u32,
        flags: u32,
        s: *const u16,
        n: i32,
        out: *mut u8,
        cap: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut SecurityAttributes,
        create: u32,
        flags: u32,
        template: *mut c_void,
    ) -> isize;
    fn DeleteFileW(name: *const u16) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}

/// A C file name (up to its first NUL) as the NUL-terminated UTF-16 path
/// the system takes: `__acrt_mbs_to_wcs_cp` with the ANSI code page, which
/// refuses bytes the code page has no character for (EILSEQ).
pub(crate) fn wide(name: &[u8]) -> std::io::Result<Vec<u16>> {
    let name = super::c_str(name);
    let mut out = vec![0u16; name.len() + 1];
    if name.is_empty() {
        return Ok(out);
    }
    // SAFETY: `name` is readable for its length and `out` writable for its capacity, which holds every unit a byte can become
    let n = unsafe {
        MultiByteToWideChar(
            GetACP(),
            MB_PRECOMPOSED | MB_ERR_INVALID_CHARS,
            name.as_ptr(),
            name.len() as i32,
            out.as_mut_ptr(),
            out.len() as i32,
        )
    };
    if n == 0 {
        return Err(std::io::Error::last_os_error());
    }
    out.truncate(n as usize);
    out.push(0);
    Ok(out)
}

/// A C file name as an OS path, through the ANSI code page; bytes the code
/// page refuses are read as UTF-8 instead, for a path nothing can open.
pub(crate) fn os_path(name: &[u8]) -> std::path::PathBuf {
    match wide(name) {
        Ok(mut w) => {
            w.pop();
            OsString::from_wide(&w).into()
        }
        Err(_) => String::from_utf8_lossy(super::c_str(name))
            .into_owned()
            .into(),
    }
}

/// What the system's UTF-16 text is to a narrow function: the ANSI code
/// page's bytes, `?` for a character it has none for (`WideCharToMultiByte`
/// without flags).
pub(crate) fn narrow(s: &OsStr) -> Vec<u8> {
    let w: Vec<u16> = s.encode_wide().collect();
    if w.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; 4 * w.len()];
    // SAFETY: `w` is readable for its length and `out` writable for its capacity, four bytes a unit being the most any code page needs
    let n = unsafe {
        WideCharToMultiByte(
            GetACP(),
            0,
            w.as_ptr(),
            w.len() as i32,
            out.as_mut_ptr(),
            out.len() as i32,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    out.truncate(n.max(0) as usize);
    out
}

/// `CreateFileW`, the handle inheritable when `inherit` is true.
pub(super) fn create_file(
    path: &[u16],
    access: u32,
    share: u32,
    inherit: bool,
    create: u32,
    flags: u32,
) -> std::io::Result<std::fs::File> {
    let mut sa = SecurityAttributes {
        length: size_of::<SecurityAttributes>() as u32,
        descriptor: std::ptr::null_mut(),
        inherit: i32::from(inherit),
    };
    // SAFETY: `path` is NUL-terminated (from `wide`) and `sa` lives across the call
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            share,
            &mut sa,
            create,
            flags,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `h` is a handle `CreateFileW` just opened, owned by nothing else
    Ok(unsafe { std::fs::File::from_raw_handle(h as *mut c_void) })
}

/// `_wremove`: `DeleteFileW`, which does not remove a directory.
pub(crate) fn remove(name: &[u8]) -> std::io::Result<()> {
    let path = wide(name)?;
    // SAFETY: `path` is NUL-terminated
    if unsafe { DeleteFileW(path.as_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// `_wrename`: `MoveFileExW` allowing a copy, which does not replace an
/// existing file.
pub(crate) fn rename(from: &[u8], to: &[u8]) -> std::io::Result<()> {
    let (f, t) = (wide(from)?, wide(to)?);
    // SAFETY: both paths are NUL-terminated
    if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), MOVEFILE_COPY_ALLOWED) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
