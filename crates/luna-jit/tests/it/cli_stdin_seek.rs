//! `seek` on the standard streams of the `luna` command on Windows, where
//! the MSVC C library's `fseek` and `ftell` go to the system for standard
//! input as for any file: a file redirected in can be sought, and `NUL` is
//! at 0. Run as `cmd` runs them, against what PUC 5.1.5 to 5.5.0 built
//! with MSVC gave on windows-latest (5.2 to 5.5 give the same). A pipe is
//! left out: what the system reports for it depends on how much the
//! writer has put in it yet.
#![cfg(windows)]

use crate::cli_common::{luna, workdir};
use std::process::Command;

const SCRIPT: &str = include_str!("../../../luna-core/tests/crt_text/stdinseek.lua");
const PUC_51: &str = include_str!("../../../luna-core/tests/crt_text/stdinseek.5.1.txt");
const PUC: &str = include_str!("../../../luna-core/tests/crt_text/stdinseek.5.4.txt");

#[test]
fn standard_streams_seek_as_in_puc_built_with_msvc() {
    for d in ["5.1", "5.2", "5.3", "5.4", "5.5"] {
        let want = if d == "5.1" { PUC_51 } else { PUC };
        let dir = workdir(&[("seek.lua", SCRIPT), ("in.txt", "line one\nline two\n")]);
        let exe = luna();
        let q = format!("\"{}\" --lua={d} seek.lua seek.out", exe.display());
        for (label, redirect) in [
            ("file", format!("{q} file < in.txt > so.txt 2> se.txt")),
            ("nul", format!("{q} nul < NUL > so.txt 2> se.txt")),
        ] {
            // the command line as `cmd` reads it, quotes and all
            use std::os::windows::process::CommandExt;
            let status = Command::new("cmd")
                .raw_arg(format!("/c {redirect}"))
                .current_dir(&dir)
                .status()
                .expect("run cmd");
            let stderr = std::fs::read_to_string(dir.join("se.txt")).unwrap_or_default();
            assert!(status.success(), "--lua={d} {label}: {status}\n{stderr}");
        }
        let got = std::fs::read_to_string(dir.join("seek.out")).expect("the results");
        std::fs::remove_dir_all(&dir).expect("remove the work dir");
        let want = want.replace("\r\n", "\n");
        for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
            assert_eq!(g, w, "--lua={d}, line {}", i + 1);
        }
        assert_eq!(
            got.lines().count(),
            want.lines().count(),
            "--lua={d}: line count"
        );
    }
}
