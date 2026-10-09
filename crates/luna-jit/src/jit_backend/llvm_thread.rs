//! The LLVM backend's compile thread: LLVM takes milliseconds per
//! function, so hot code keeps running on quicker code while LLVM compiles
//! it here.
//!
//! A job may wait for a moment before it starts ([`submit`]'s `not_before`),
//! so code that stops being hot meanwhile, or a program that ends, never
//! pays for it. A Vm cancels its jobs as it goes away ([`cancel_and_wait`])
//! and waits for the one LLVM may be running: LLVM must not be running when
//! the process exits and its global state is torn down.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// Cancels a submitted job (see [`cancel_and_wait`]).
pub(crate) type Ticket = Arc<AtomicBool>;

struct Job {
    run: Box<dyn FnOnce() + Send>,
    not_before: Instant,
    cancelled: Ticket,
}

struct State {
    queue: Vec<Job>,
    /// Jobs running now (0 or 1).
    running: usize,
    started: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    queue: Vec::new(),
    running: 0,
    started: false,
});
static WAKE: Condvar = Condvar::new();

const POISON: &str = "the compile thread never panics holding its lock";

/// Runs `run` on the compile thread, not before `not_before`.
pub(crate) fn submit(run: Box<dyn FnOnce() + Send>, not_before: Instant) -> Ticket {
    let cancelled = Ticket::default();
    let mut st = STATE.lock().expect(POISON);
    if !st.started {
        st.started = true;
        std::thread::Builder::new()
            .name("luna-llvm".into())
            .spawn(work)
            .expect("starting the LLVM compile thread");
    }
    st.queue.push(Job {
        run,
        not_before,
        cancelled: cancelled.clone(),
    });
    WAKE.notify_all();
    cancelled
}

/// Cancels the jobs of `tickets` that have not started and waits until
/// no job is running.
pub(crate) fn cancel_and_wait(tickets: &[Ticket]) {
    let mut st = STATE.lock().expect(POISON);
    for t in tickets {
        t.store(true, Ordering::Relaxed);
    }
    st.queue.retain(|j| !j.cancelled.load(Ordering::Relaxed));
    WAKE.notify_all();
    while st.running > 0 {
        st = WAKE.wait(st).expect(POISON);
    }
}

fn work() {
    let mut st = STATE.lock().expect(POISON);
    loop {
        st.queue.retain(|j| !j.cancelled.load(Ordering::Relaxed));
        let now = Instant::now();
        let ready = st.queue.iter().position(|j| j.not_before <= now);
        let Some(k) = ready else {
            st = match st.queue.iter().map(|j| j.not_before).min() {
                Some(t) => WAKE.wait_timeout(st, t - now).expect(POISON).0,
                None => WAKE.wait(st).expect(POISON),
            };
            continue;
        };
        let job = st.queue.remove(k);
        st.running += 1;
        drop(st);
        (job.run)();
        st = STATE.lock().expect(POISON);
        st.running -= 1;
        WAKE.notify_all();
    }
}

/// Whether code this thread just wrote can be handed to another thread to
/// run. On aarch64 a core that runs code another core wrote must first
/// resynchronise its instruction fetch (an `isb`); the writer has cleaned
/// and invalidated the caches already (the execution engine does), and
/// Linux's `membarrier` makes every core of the process resynchronise. Where
/// that is not available the code is not handed over.
pub(crate) fn cores_synced() -> bool {
    sync_cores()
}

#[cfg(not(target_arch = "aarch64"))]
fn sync_cores() -> bool {
    // x86 keeps instruction fetch coherent with stores
    true
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn sync_cores() -> bool {
    // linux/membarrier.h
    const REGISTER_SYNC_CORE: libc::c_long = 1 << 6;
    const SYNC_CORE: libc::c_long = 1 << 5;
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let registered = *REGISTERED.get_or_init(|| {
        // SAFETY: membarrier takes two integers and touches no memory
        unsafe { libc::syscall(libc::SYS_membarrier, REGISTER_SYNC_CORE, 0) == 0 }
    });
    // SAFETY: as above
    registered && unsafe { libc::syscall(libc::SYS_membarrier, SYNC_CORE, 0) == 0 }
}

#[cfg(all(target_arch = "aarch64", not(target_os = "linux")))]
fn sync_cores() -> bool {
    false
}
