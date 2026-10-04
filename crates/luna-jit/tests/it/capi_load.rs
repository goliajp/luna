//! C hosts of the C API: load (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn load_reader() {
    check("load_reader");
}

#[test]
fn load_binary() {
    check("load_binary");
}

#[test]
fn load_ident() {
    check("load_ident");
}

#[test]
fn gc_ops() {
    check("gc_ops");
}

#[test]
fn gc_close() {
    check("gc_close");
}

#[test]
fn gc_alloc() {
    check("gc_alloc");
}

#[test]
fn alloc_count() {
    check("alloc_count");
}

#[test]
fn warn_default() {
    check("warn_default");
}

#[test]
fn warn_custom() {
    check("warn_custom");
}

#[test]
fn panic_string() {
    check("panic_string");
}

#[test]
fn panic_number() {
    check("panic_number");
}

#[test]
fn panic_table() {
    check("panic_table");
}

#[test]
fn panic_lua() {
    check("panic_lua");
}

#[test]
fn panic_cfunction() {
    check("panic_cfunction");
}

#[test]
fn panic_custom() {
    check("panic_custom");
}

#[test]
fn panic_jump() {
    check("panic_jump");
}

#[test]
fn panic_newstate() {
    check("panic_newstate");
}

#[test]
fn panic_close() {
    check("panic_close");
}

#[test]
fn dump_writer() {
    check("dump_writer")
}

#[test]
fn load_stream() {
    check("load_stream")
}
