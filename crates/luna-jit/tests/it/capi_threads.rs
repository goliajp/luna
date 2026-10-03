//! C hosts of the C API: threads (see `capi_hosts`).

use super::capi_hosts::check;

#[test]
fn threads_new() {
    check("threads_new")
}

#[test]
fn threads_resume() {
    check("threads_resume")
}

#[test]
fn threads_close() {
    check("threads_close")
}

#[test]
fn cont_callk() {
    check("cont_callk")
}

#[test]
fn cont_pcallk() {
    check("cont_pcallk")
}

#[test]
fn cont_lua() {
    check("cont_lua")
}
