use object::{Object, ObjectSection, ObjectSymbol, RelocationTarget};

use luna_core::version::LuaVersion;

use super::super::HarvestedTraces;
use super::super::compile_to_dump;
use super::super::target::TargetSpec;
use super::harvest_and_emit_aot_traces;

// a hot loop whose trace reads and writes two string-keyed fields of a
// table, so the trace lowerer routes both keys through the strkey slots
const FIELD_LOOP: &[u8] = b"local t = { hits = 0, total = 0 }\n\
    for i = 1, 100000 do\n\
      t.hits = t.hits + 1\n\
      t.total = t.total + i\n\
    end\n\
    print(t.hits, t.total)\n";

const KEYS: [&str; 2] = ["hits", "total"];

// same label as the trace lowerer's `strkey_hex_label`
fn strkey_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn harvest_object_for(triple: &str) -> Vec<u8> {
    let td = tempfile::tempdir().expect("tempdir");
    let src = td.path().join("fields.lua");
    std::fs::write(&src, FIELD_LOOP).expect("write source");
    let dump = compile_to_dump(&src, LuaVersion::Lua55).expect("compile source");
    let target = TargetSpec::from_triple(triple).expect("target triple");
    let out = td.path().join("traces.o");
    let harvested = harvest_and_emit_aot_traces(&dump, LuaVersion::Lua55, &out, &target, false)
        .unwrap_or_else(|e| panic!("harvest for {triple}: {e}"));
    assert!(
        matches!(harvested, HarvestedTraces::Some),
        "{triple}: the field loop must leave at least one AOT trace"
    );
    std::fs::read(&out).expect("read trace object")
}

/// The data bytes a symbol covers, read from its section.
fn symbol_bytes<'d>(obj: &object::File<'d>, sym: &object::Symbol<'d, '_>, len: usize) -> &'d [u8] {
    let section = obj
        .section_by_index(sym.section_index().expect("symbol has a section"))
        .expect("symbol section");
    let off = (sym.address() - section.address()) as usize;
    &section.data().expect("section data")[off..off + len]
}

/// Checks the strkey index of a trace object for one target format: one
/// 16-byte entry per key in sections of that name (and Mach-O segment),
/// each entry with two relocations, to the key's bytes manifest and to
/// its slot. The linker merges the sections into the one the deploy side
/// brackets. Returns a line describing the sections.
fn check_strkey_index(triple: &str, want_name: &str, want_segment: Option<&str>) -> String {
    let bytes = harvest_object_for(triple);
    let obj = object::File::parse(&*bytes).expect("parse trace object");
    let sections: Vec<_> = obj
        .sections()
        .filter(|s| s.name() == Ok(want_name))
        .collect();
    let names: Vec<_> = obj.sections().filter_map(|s| s.name().ok()).collect();
    assert!(
        !sections.is_empty(),
        "{triple}: no `{want_name}` section; sections: {names:?}"
    );
    let total: u64 = sections.iter().map(|s| s.size()).sum();
    assert_eq!(
        total,
        16 * KEYS.len() as u64,
        "{triple}: one 16-byte entry per key"
    );

    // every entry is [bytes_addr, slot_addr], both filled by the linker
    let mut targets: Vec<String> = Vec::new();
    for idx in &sections {
        if let Some(seg) = want_segment {
            assert_eq!(
                idx.segment_name().ok().flatten(),
                Some(seg),
                "{triple}: strkey index must sit in segment {seg}"
            );
        }
        assert_eq!(idx.size() % 16, 0, "{triple}: whole 16-byte entries");
        assert!(
            idx.align() >= 8,
            "{triple}: pointer slots need 8-byte alignment"
        );
        let mut relocs: Vec<(u64, String)> = idx
            .relocations()
            .map(|(off, r)| {
                let RelocationTarget::Symbol(si) = r.target() else {
                    panic!("{triple}: strkey index relocation must target a symbol")
                };
                let name = obj.symbol_by_index(si).expect("reloc symbol").name();
                (off, name.expect("symbol name").to_owned())
            })
            .collect();
        relocs.sort();
        assert_eq!(relocs.len() as u64, idx.size() / 8, "{triple}: {relocs:?}");
        for pair in relocs.chunks(2) {
            let [(bytes_off, bytes_sym), (slot_off, slot_sym)] = pair else {
                unreachable!("even count checked above")
            };
            assert_eq!(bytes_off % 16, 0, "{triple}: {relocs:?}");
            assert_eq!(*slot_off, bytes_off + 8, "{triple}: {relocs:?}");
            let hex = bytes_sym
                .rsplit_once("luna_aot_strkey_bytes_")
                .unwrap_or_else(|| panic!("{triple}: entry starts with {bytes_sym}"))
                .1;
            assert!(
                slot_sym.ends_with(&format!("luna_aot_strkey_slot_{hex}")),
                "{triple}: entry pairs {bytes_sym} with {slot_sym}"
            );
            targets.push(bytes_sym.clone());
        }
    }

    // each key's bytes manifest is [u64 len][bytes] under its own label
    for key in KEYS {
        let hex = strkey_hex(key.as_bytes());
        let sym = obj
            .symbols()
            .find(|s| {
                s.name()
                    .is_ok_and(|n| n.ends_with(&format!("luna_aot_strkey_bytes_{hex}")))
            })
            .unwrap_or_else(|| panic!("{triple}: no bytes manifest for {key:?}"));
        let data = symbol_bytes(&obj, &sym, 8 + key.len());
        assert_eq!(
            data[..8],
            (key.len() as u64).to_le_bytes(),
            "{triple}: {key:?} length"
        );
        assert_eq!(&data[8..], key.as_bytes(), "{triple}: {key:?} bytes");
        assert!(
            targets.iter().any(|n| n == sym.name().unwrap()),
            "{triple}: the index has no entry for {key:?}"
        );
    }

    let shown = match want_segment {
        Some(seg) => format!("{seg},{want_name}"),
        None => want_name.to_owned(),
    };
    let sizes: Vec<u64> = sections.iter().map(|s| s.size()).collect();
    format!(
        "{triple}: {:?}, sections `{shown}` sizes {sizes:?} align {}, {} entries",
        obj.format(),
        sections[0].align(),
        targets.len()
    )
}

#[test]
fn strkey_index_section_is_named_for_each_object_format() {
    let lines = [
        check_strkey_index("x86_64-unknown-linux-gnu", "luna_strkey_idx", None),
        check_strkey_index("aarch64-apple-darwin", "luna_strkey_idx", Some("__DATA")),
        check_strkey_index("x86_64-pc-windows-msvc", ".lt_skix", None),
    ];
    for line in lines {
        eprintln!("{line}");
    }
}
