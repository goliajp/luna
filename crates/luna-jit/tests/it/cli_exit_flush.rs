//! What the `luna` CLI leaves open writes out when it exits, as `lua.c`
//! closing the state and C's `exit` flushing every stream do: a file and
//! a pipe still open, the default output set to a file, each also with
//! the `__gc` of `FILE*` taken away (so closing the state leaves them
//! open). Recorded from the stock PUC interpreters: every byte arrives.

use std::process::Command;

use crate::cli_common::{DIALECTS, luna, workdir};

const SCRIPT: &str = r#"
local keep = ...
if keep == "nogc" then getmetatable(io.stdout).__gc = nil end
local f = io.open("file.txt", "w")
f:write("file")
local p = io.popen("cat > pipe.txt", "w")
p:write("pipe")
io.output("out.txt")
io.write("out")
"#;

#[cfg(unix)]
#[test]
fn open_streams_write_out_at_exit() {
    for d in DIALECTS {
        for keep in ["gc", "nogc"] {
            let dir = workdir(&[("s.lua", SCRIPT)]);
            let st = Command::new(luna())
                .arg(format!("--lua={d}"))
                .args(["s.lua", keep])
                .current_dir(&dir)
                .status()
                .expect("run luna");
            assert!(st.success(), "{d} {keep}");
            // `cat` writes its file once it reads the pipe's end
            let read = |name: &str| {
                for _ in 0..200 {
                    if let Ok(s) = std::fs::read_to_string(dir.join(name))
                        && !s.is_empty()
                    {
                        return s;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                String::new()
            };
            assert_eq!(read("file.txt"), "file", "{d} {keep}");
            assert_eq!(read("pipe.txt"), "pipe", "{d} {keep}");
            assert_eq!(read("out.txt"), "out", "{d} {keep}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
