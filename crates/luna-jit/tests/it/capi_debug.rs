//! C hosts of the C API: debug (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn debug_info() {
    check("debug_info")
}

#[test]
fn debug_locals() {
    check("debug_locals")
}

#[test]
fn debug_upvalues() {
    check("debug_upvalues")
}

#[test]
fn hook_events() {
    check("hook_events")
}

#[test]
fn hook_yield() {
    check("hook_yield")
}

#[test]
fn hook_transfer() {
    check("hook_transfer")
}
