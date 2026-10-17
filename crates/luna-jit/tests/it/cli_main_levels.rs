//! The stack a script's main chunk sees below it: lua.c's `docall` calls
//! the chunk from `pmain` with `pmain`'s arguments (`argc` and `argv`, 5.1
//! the light userdata of `lua_cpcall`) and the message handler left under
//! the chunk, and 5.4 on a vararg chunk leaves its old slot and its
//! arguments there too. Recorded from the stock PUC 5.1.5 to 5.5.1
//! interpreters as `lua main.lua a b`.

use crate::cli_common::{as_on_this_platform, run, workdir};

const SCRIPT: &str = r##"print(debug.traceback("tb"))
for l = 0, 4 do
  local i = debug.getinfo(l, "Sln")
  if not i then print(l, "nil") break end
  print(l, i.what, i.short_src, i.currentline, i.name)
end
for i = 1, 8 do
  local name, v = debug.getlocal(2, i)
  if not name then print(i, "none") break end
  print(i, name, type(v) == "string" and v or type(v))
end
print(select("#", ...), ...)
"##;

const LEVELS: &str =
    "0\tC\t[C]\t-1\tgetinfo\n1\tmain\tmain.lua\t3\tnil\n2\tC\t[C]\t-1\tnil\n3\tnil\n";

#[test]
fn main_chunk_sees_pmain_below_it() {
    let dir = workdir(&[("main.lua", SCRIPT)]);
    let tb = |caller: &str| {
        format!("tb\nstack traceback:\n\tmain.lua:1: in main chunk\n\t[C]: {caller}\n{LEVELS}")
    };
    let t = "(*temporary)";
    let c = "(C temporary)";
    let cases = [
        (
            "5.1",
            format!("{}1\t{t}\tuserdata\n2\t{t}\tfunction\n3\tnone\n", tb("?")),
        ),
        (
            "5.2",
            format!(
                "{}1\t{t}\tnumber\n2\t{t}\tuserdata\n3\t{t}\tfunction\n4\tnone\n",
                tb("in ?")
            ),
        ),
        (
            "5.3",
            format!(
                "{}1\t{t}\tnumber\n2\t{t}\tuserdata\n3\t{t}\tfunction\n4\tnone\n",
                tb("in ?")
            ),
        ),
        (
            "5.4",
            format!(
                "{}1\t{c}\tnumber\n2\t{c}\tuserdata\n3\t{c}\tfunction\n4\t{c}\tfunction\n5\t{c}\ta\n6\t{c}\tb\n7\tnone\n",
                tb("in ?")
            ),
        ),
        (
            "5.5",
            format!(
                "{}1\t{c}\tnumber\n2\t{c}\tuserdata\n3\t{c}\tfunction\n4\t{c}\tfunction\n5\t{c}\ta\n6\t{c}\tb\n7\tnone\n",
                tb("in ?")
            ),
        ),
    ];
    for (d, stdout) in cases {
        let out = run(d, &dir, &["main.lua", "a", "b"], None, &[]);
        let stdout = format!("{stdout}2\ta\tb\n");
        assert_eq!(
            out.stdout,
            as_on_this_platform(&stdout),
            "stdout, --lua={d}"
        );
        assert_eq!(out.stderr, "", "stderr, --lua={d}");
        assert_eq!(out.status, 0);
    }
    std::fs::remove_dir_all(&dir).expect("remove the work dir");
}
