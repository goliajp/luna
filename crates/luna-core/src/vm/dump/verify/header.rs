//! The per-function checks that need no instruction decoding.

use crate::runtime::function::Proto;

pub(super) fn check_header(p: &Proto, parent: Option<&Proto>) -> Result<(), String> {
    if p.code.is_empty() {
        return Err("no instructions".to_string());
    }
    if p.num_params > p.max_stack {
        return Err(format!(
            "{} parameters exceed stack size {}",
            p.num_params, p.max_stack
        ));
    }
    if p.has_compat_vararg_arg && p.num_params >= p.max_stack {
        return Err(format!(
            "no register for 'arg' after {} parameters (stack size {})",
            p.num_params, p.max_stack
        ));
    }
    if !p.lines.is_empty() && p.lines.len() != p.code.len() {
        return Err(format!(
            "{} line entries for {} instructions",
            p.lines.len(),
            p.code.len()
        ));
    }
    // the debug library reads and writes a named local at its register
    if let Some(v) = p.locvars.iter().find(|v| v.reg >= u32::from(p.max_stack)) {
        return Err(format!(
            "local '{}' in register {} out of range (stack size {})",
            v.name, v.reg, p.max_stack
        ));
    }
    let Some(parent) = parent else {
        return Ok(());
    };
    for (i, u) in p.upvals.iter().enumerate() {
        let (limit, what) = if u.in_stack {
            (parent.max_stack as usize, "register")
        } else {
            (parent.upvals.len(), "enclosing upvalue")
        };
        if u.index as usize >= limit {
            return Err(format!(
                "upvalue {} captures {what} {} out of range (limit {limit})",
                i + 1,
                u.index
            ));
        }
    }
    Ok(())
}
