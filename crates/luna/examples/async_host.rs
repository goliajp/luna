//! Async embedding walkthrough (A7 artifact 5 / B10 example).
//!
//! Demonstrates:
//! - `vm.eval_async(src)` driving a Lua script with cooperative yields
//! - `vm.set_async_native(name, fn)` exposing an async Rust function
//!   to Lua scripts
//! - Hand-rolled `block_on` so the example is self-contained (no
//!   tokio dev-dep)
//!
//! Run: `cargo run --example async_host -p luna`
//!
//! Embedders integrating with tokio swap `block_on` for
//! `#[tokio::main(flavor = "current_thread")]` or a `LocalSet`;
//! see `docs/threading.md` for the canonical patterns.

use luna::Lua;
use luna_core::runtime::Value;
use luna_core::vm::LuaError;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// Minimal hand-rolled `block_on`. Spins on `Pending` (busy-wait) —
/// fine for the demo because the async natives below always resolve
/// immediately. Real embedders use tokio / async-std / a proper
/// executor.
fn block_on<F: Future>(mut fut: F) -> F::Output {
    // SAFETY: `fut` is owned by this stack frame and not moved
    // again — the pin is local.
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => {} // busy-loop; the demo's futures don't truly suspend
        }
    }
}

fn noop_waker() -> Waker {
    use std::task::{RawWaker, RawWakerVTable};
    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(|_| RawWaker::new(std::ptr::null(), &VTABLE), |_| {}, |_| {}, |_| {});
    let raw = RawWaker::new(std::ptr::null(), &VTABLE);
    // SAFETY: VTABLE is a `'static` and all four entries are no-ops
    // returning either another raw or unit; the data pointer is null
    // and never dereferenced.
    unsafe { Waker::from_raw(raw) }
}

/// An async native function. Receives a `*mut Vm` (the type-erased
/// pointer the dispatcher hands to async natives) plus the Lua-side
/// arg slots, returns a boxed `!Send` future producing the count of
/// values pushed back to the Lua stack.
///
/// This one reads one Lua-side i64, doubles it, returns it. In a
/// real host the future would call `tokio::net::TcpStream::read`,
/// `sqlx::query`, etc. — anything that actually awaits I/O.
fn async_double(
    vm: *mut luna_core::vm::Vm,
    fs: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        // SAFETY: the dispatcher hands the *mut Vm to the async
        // native; per AsyncNativeFn's contract the pointer is live
        // until the returned future resolves and we don't reborrow
        // it across an .await suspension. Here we read once + write
        // once before returning.
        let vm = unsafe { &mut *vm };
        let arg = vm.nat_arg(fs, 1, 0);
        let n = match arg {
            Value::Int(i) => i,
            Value::Float(f) => f as i64,
            _ => return Err(LuaError(Value::Nil)),
        };
        // Pretend we did some async work here — in a real embedder:
        //     let row = sqlx::query!(...).fetch_one(&pool).await?;
        // The future returned by Box::pin above is what the
        // dispatcher awaits.
        let doubled = n * 2;
        Ok(vm.nat_return(fs, &[Value::Int(doubled)]))
    })
}

fn main() {
    let mut lua = Lua::new();
    lua.open_base();
    lua.open_math();
    lua.open_string();

    // Register the async native.
    lua.set_async_native("double_async", async_double).unwrap();

    // Drive a Lua script through eval_async. The dispatcher
    // cooperatively yields whenever the instruction budget hits 0;
    // the async native calls suspend through the same machinery.
    let result: i64 = block_on(lua.eval_async(
        r#"
            local sum = 0
            for i = 1, 10 do
                sum = sum + double_async(i)
            end
            return sum
        "#,
    ))
    .unwrap();

    // 2 + 4 + 6 + ... + 20 = 110
    assert_eq!(result, 110);
    println!("async_host: sum = {result}");
    println!("ok");
}
