use luna::runtime::Value;
use luna::version::LuaVersion;
use luna::vm::Vm;

fn main() {
    std::env::set_current_dir("tests/official/lua-5.5.0-tests").unwrap();
    let mut files: Vec<_> = std::fs::read_dir(".")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    files.sort();
    let skip = ["all.lua", "main.lua", "heavy.lua", "big.lua", "verybig.lua"];
    let (mut pass, mut fail) = (0, 0);
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        if skip.contains(&name.as_str()) {
            continue;
        }
        let src = std::fs::read(f).unwrap();
        let label = name.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || {
                let mut vm = Vm::new(LuaVersion::Lua55);
                vm.set_global("_U", Value::Bool(true));
                let chunkname = format!("@{label}");
                let r = match vm.load(&src, chunkname.as_bytes()) {
                    Ok(cl) => match vm.call_value(Value::Closure(cl), &[]) {
                        Ok(_) => "PASS".to_string(),
                        Err(e) => format!("FAIL {:.130}", vm.error_text(&e)),
                    },
                    Err(e) => format!("COMPILE {e}"),
                };
                let _ = tx.send(r);
            })
            .unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(r) => {
                if r == "PASS" {
                    pass += 1;
                } else {
                    fail += 1;
                }
                eprintln!("{r:6.140} <= {name}");
            }
            Err(_) => {
                fail += 1;
                eprintln!("HANG   <= {name}");
            }
        }
    }
    eprintln!("== {pass} pass, {fail} fail");
}
