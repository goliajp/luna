//! A program attached to a Windows pseudo console (ConPTY), so that its
//! standard streams are a console: what it writes, and keys typed to it.
//! What the console shows is the screen ConPTY's bytes paint
//! (`cli_console_screen`), not the bytes themselves.
//!
//! With `LUNA_CONSOLE_RAW_DIR` set, each console's bytes are also written
//! to a file there when it closes, to see which sequences ConPTY sends.

use crate::cli_common::{luna, workdir};
use crate::cli_console_screen::Screen;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

/// The console's size.
const WIDTH: usize = 120;
const HEIGHT: usize = 30;

/// A child process on its own pseudo console.
pub(crate) struct Console {
    pc: HPCON,
    input: HANDLE,
    process: HANDLE,
    thread: HANDLE,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

fn pipe() -> (HANDLE, HANDLE) {
    let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: both out-pointers are valid; default security, default size
    let ok = unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) };
    assert_ne!(ok, 0, "CreatePipe: {}", std::io::Error::last_os_error());
    (read, write)
}

/// `KEY=value\0...\0\0` of this process's environment with `overrides`.
fn environment(overrides: &[(&str, &OsString)]) -> Vec<u16> {
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !overrides.iter().any(|(o, _)| k.eq_ignore_ascii_case(*o)))
        .collect();
    vars.extend(overrides.iter().map(|(k, v)| ((*k).into(), (*v).clone())));
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

impl Console {
    /// `luna` with `args` on a console of its own.
    pub(crate) fn spawn(args: &[&str]) -> Console {
        Console::spawn_program(&luna(), args)
    }

    /// `program` with `args` on a console of its own, in a fresh directory.
    pub(crate) fn spawn_program(program: &std::path::Path, args: &[&str]) -> Console {
        let (pc_in, input) = pipe();
        let (output_read, pc_out) = pipe();
        let mut pc: HPCON = 0;
        let size = COORD {
            X: WIDTH as i16,
            Y: HEIGHT as i16,
        };
        // SAFETY: the pipe ends are open; `pc` receives the console
        let hr = unsafe { CreatePseudoConsole(size, pc_in, pc_out, 0, &mut pc) };
        assert_eq!(hr, 0, "CreatePseudoConsole: {hr:#x}");

        let mut attr_size = 0usize;
        // SAFETY: the size query; it fails by design and sets `attr_size`
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size) };
        let mut attrs = vec![0u8; attr_size];
        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // the handles stay null: the child must not inherit this process's
        // redirected stdio, only the pseudo console
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.lpAttributeList = attrs.as_mut_ptr().cast();
        // SAFETY: `attrs` has the size the query asked for and outlives the
        // CreateProcessW call; `pc` stays open until `Drop`
        unsafe {
            assert_ne!(
                InitializeProcThreadAttributeList(si.lpAttributeList, 1, 0, &mut attr_size),
                0,
                "InitializeProcThreadAttributeList"
            );
            assert_ne!(
                UpdateProcThreadAttribute(
                    si.lpAttributeList,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    pc as *const std::ffi::c_void,
                    std::mem::size_of::<HPCON>(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                ),
                0,
                "UpdateProcThreadAttribute"
            );
        }

        let mut cmdline: Vec<u16> = format!("\"{}\"", program.display())
            .encode_utf16()
            .collect();
        for a in args {
            cmdline.extend(format!(" {a}").encode_utf16());
        }
        cmdline.push(0);
        // a history file of its own, not the user's
        let home = workdir(&[]).into_os_string();
        let env = environment(&[("HOME", &home)]);
        let cwd: Vec<u16> = home.encode_wide().chain([0]).collect();
        let mut pi = PROCESS_INFORMATION::default();
        // SAFETY: every pointer is valid for the call; the command line is
        // writable as CreateProcessW requires
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env.as_ptr().cast(),
                cwd.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            )
        };
        let err = std::io::Error::last_os_error();
        // SAFETY: the list was initialized above; the console holds its own
        // copies of its pipe ends
        unsafe {
            DeleteProcThreadAttributeList(si.lpAttributeList);
            CloseHandle(pc_in);
            CloseHandle(pc_out);
        }
        assert_ne!(ok, 0, "CreateProcessW: {err}");

        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = output.clone();
        let read_end = output_read as usize;
        // the console blocks when its output is not drained, so a thread
        // reads it until the console closes
        let reader = std::thread::spawn(move || {
            let h = read_end as HANDLE;
            let mut buf = [0u8; 4096];
            loop {
                let mut n = 0u32;
                // SAFETY: `buf` is writable for its length
                let ok = unsafe {
                    ReadFile(
                        h,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut n,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 || n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n as usize]);
            }
            // SAFETY: this thread owns the read end
            unsafe { CloseHandle(h) };
        });
        Console {
            pc,
            input,
            process: pi.hProcess,
            thread: pi.hThread,
            output,
            reader: Some(reader),
        }
    }

    pub(crate) fn type_keys(&self, keys: &str) {
        let mut n = 0u32;
        // SAFETY: `keys` is readable for its length
        let ok = unsafe {
            WriteFile(
                self.input,
                keys.as_ptr(),
                keys.len() as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        };
        assert!(
            ok != 0 && n as usize == keys.len(),
            "WriteFile: {}",
            std::io::Error::last_os_error()
        );
    }

    /// What the console shows so far: its rows, each ended by `\r\n` but
    /// the last (see [`Screen::text`]).
    pub(crate) fn screen(&self) -> String {
        Screen::render(&self.output.lock().unwrap(), WIDTH, HEIGHT).text()
    }

    /// Waits until what the console shows holds `text` after `from` (a
    /// position in [`Console::screen`]); returns where it ends.
    pub(crate) fn wait_for(&self, text: &str, from: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let screen = self.screen();
            if let Some(at) = screen.get(from..).and_then(|s| s.find(text)) {
                return from + at + text.len();
            }
            assert!(
                Instant::now() < deadline,
                "no {text:?} on the console; it shows {screen:?}, raw {:?}",
                String::from_utf8_lossy(&self.output.lock().unwrap())
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The output once the console has gone quiet: what it shows after
    /// nothing new has arrived for a while (the program has exited; its
    /// last output is still crossing the console's pipe).
    pub(crate) fn settled_screen(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut last = self.output.lock().unwrap().len();
        let mut quiet_since = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let now = self.output.lock().unwrap().len();
            if now != last {
                last = now;
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() >= Duration::from_millis(500) {
                return self.screen();
            }
            assert!(Instant::now() < deadline, "the console keeps writing");
        }
    }

    pub(crate) fn exit_code(&self) -> u32 {
        // SAFETY: the process handle is open
        let waited = unsafe { WaitForSingleObject(self.process, 60_000) };
        assert_eq!(
            waited,
            WAIT_OBJECT_0,
            "the program did not exit; console shows {:?}",
            self.screen()
        );
        let mut code = 0u32;
        // SAFETY: the process handle is open
        assert_ne!(unsafe { GetExitCodeProcess(self.process, &mut code) }, 0);
        code
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        // SAFETY: the handles are open and owned here; the reader is still
        // draining the output, so closing the console cannot block on it
        unsafe {
            TerminateProcess(self.process, 1);
            ClosePseudoConsole(self.pc);
            CloseHandle(self.input);
            CloseHandle(self.process);
            CloseHandle(self.thread);
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        if let Some(dir) = std::env::var_os("LUNA_CONSOLE_RAW_DIR") {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let name = format!(
                "console-{}-{}.raw",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            );
            let bytes = self.output.lock().unwrap_or_else(|p| p.into_inner());
            let _ = std::fs::write(std::path::Path::new(&dir).join(name), &*bytes);
        }
    }
}

/// `s` without its CSI and OSC sequences.
pub(crate) fn strip_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                // parameters and intermediates up to the final byte
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                // up to BEL or ST
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
