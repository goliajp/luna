//! C hosts of the C API: aux (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn aux_fmt() {
    check("aux_fmt");
}

#[test]
fn aux_check() {
    check("aux_check");
}

#[test]
fn aux_meta() {
    check("aux_meta");
}

#[test]
fn aux_buf() {
    check("aux_buf");
}

#[test]
fn aux_load() {
    check("aux_load");
}

#[test]
fn aux_trace() {
    check("aux_trace");
}

#[test]
fn lib_open() {
    check("lib_open");
}

// a signal in a wait status and reading a directory exist only on POSIX
#[cfg(unix)]
#[test]
fn aux_posix() {
    check("aux_posix");
}

#[test]
fn io_files() {
    check("io_files");
}

#[test]
fn io_stream() {
    check("io_stream");
}

// popen runs a POSIX shell
#[cfg(unix)]
#[test]
fn io_posix() {
    check("io_posix");
}
