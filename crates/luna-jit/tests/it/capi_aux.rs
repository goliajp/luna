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
