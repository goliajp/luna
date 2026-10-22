//! What PUC allocates while it creates a state (`lua_newstate`), per
//! dialect: the blocks luna has no object for (the global state with the
//! main thread, the main thread's stack and 5.1's CallInfo array, the
//! registry's array part, the string table), and the strings it makes.

use crate::runtime::dialect::Dialect;

/// The tag-method names `luaT_init` makes, in its order.
pub(crate) fn tm_names(d: Dialect) -> &'static [&'static str] {
    match d {
        Dialect::L51 => &[
            "__index",
            "__newindex",
            "__gc",
            "__mode",
            "__eq",
            "__add",
            "__sub",
            "__mul",
            "__div",
            "__mod",
            "__pow",
            "__unm",
            "__len",
            "__lt",
            "__le",
            "__concat",
            "__call",
        ],
        Dialect::L52 => &[
            "__index",
            "__newindex",
            "__gc",
            "__mode",
            "__len",
            "__eq",
            "__add",
            "__sub",
            "__mul",
            "__div",
            "__mod",
            "__pow",
            "__unm",
            "__lt",
            "__le",
            "__concat",
            "__call",
        ],
        Dialect::L53 => &[
            "__index",
            "__newindex",
            "__gc",
            "__mode",
            "__len",
            "__eq",
            "__add",
            "__sub",
            "__mul",
            "__mod",
            "__pow",
            "__div",
            "__idiv",
            "__band",
            "__bor",
            "__bxor",
            "__shl",
            "__shr",
            "__unm",
            "__bnot",
            "__lt",
            "__le",
            "__concat",
            "__call",
        ],
        Dialect::L54 | Dialect::L55 => &[
            "__index",
            "__newindex",
            "__gc",
            "__mode",
            "__len",
            "__eq",
            "__add",
            "__sub",
            "__mul",
            "__mod",
            "__pow",
            "__div",
            "__idiv",
            "__band",
            "__bor",
            "__bxor",
            "__shl",
            "__shr",
            "__unm",
            "__bnot",
            "__lt",
            "__le",
            "__concat",
            "__call",
            "__close",
        ],
    }
}

/// The reserved words `luaX_init` makes, in its order.
pub(crate) fn reserved_words(d: Dialect) -> &'static [&'static str] {
    const W51: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
        "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    const W52: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    const W55: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "global",
        "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true",
        "until", "while",
    ];
    match d {
        Dialect::L51 => W51,
        Dialect::L55 => W55,
        _ => W52,
    }
}

/// The sizes of what `lua_newstate` allocates that luna has no object for.
pub(crate) struct BootSizes {
    /// the global state with the main thread
    pub lg: usize,
    /// the main thread's stack slots
    pub stack_slots: usize,
    /// 5.1's CallInfo array, in entries
    pub ci_entries: usize,
    /// the string table's first size
    pub strtab: usize,
    /// the registry's array part
    pub registry_array: usize,
}

pub(crate) fn boot_sizes(d: Dialect) -> BootSizes {
    let lg64 = [616, 704, 1552, 1624, 1624];
    let lg32 = [348, 372, 808, 848, 852];
    let i = d as usize - 1;
    let lgwasm = [376, 400, 820, 864, 864];
    let lg = if cfg!(target_pointer_width = "64") {
        lg64[i]
    } else if cfg!(target_arch = "wasm32") {
        lgwasm[i]
    } else {
        lg32[i]
    };
    match d {
        Dialect::L51 => BootSizes {
            lg,
            stack_slots: 45,
            ci_entries: 8,
            strtab: 32,
            registry_array: 0,
        },
        Dialect::L52 => BootSizes {
            lg,
            stack_slots: 40,
            ci_entries: 0,
            strtab: 32,
            registry_array: 2,
        },
        Dialect::L53 => BootSizes {
            lg,
            stack_slots: 40,
            ci_entries: 0,
            strtab: 128,
            registry_array: 2,
        },
        Dialect::L54 => BootSizes {
            lg,
            stack_slots: 45,
            ci_entries: 0,
            strtab: 128,
            registry_array: 2,
        },
        Dialect::L55 => BootSizes {
            lg,
            stack_slots: 45,
            ci_entries: 0,
            strtab: 128,
            registry_array: 3,
        },
    }
}
