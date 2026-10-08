//! `goto`.

use super::*;

impl<'a> Compiler<'a> {
    /// Compile `goto name`: backward jump if a label is visible, else a
    /// pending forward reference in the current block.
    pub(super) fn goto_stat(&mut self, name: &'a str, line: u32) -> Result<(), SyntaxError> {
        self.last_line = line;
        // PUC's `gotostat` scans only the *current* block for an
        // already-defined backward label; unresolved gotos enter the pending
        // list and percolate outward on each `leave_block`, so an inner
        // block's later `::name::` (or the enclosing block's existing one)
        // gets matched at scope exit. Searching all ancestor blocks here would
        // make `do goto l; ::l:: end` lock onto the outer `::l::` before the
        // inner one even gets defined — goto.lua 5.2/5.3 :71 specifically
        // exercises that shadow.
        let found: Option<(usize, usize)> = self
            .lr()
            .blocks
            .last()
            .and_then(|b| b.labels.iter().rev().find(|l| l.name == name))
            .map(|l| (l.pc, l.nactive));
        if let Some((pc, nactive)) = found {
            // jumping back discards locals declared after the label
            if let Some(floor) = self.reg_floor_from_avar(nactive) {
                self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
            }
            self.jump_back(pc)?;
            return Ok(());
        }
        let jmp = self.emit_jump();
        let nactive = self.lr().avars.len();
        self.l()
            .blocks
            .last_mut()
            .expect("no block")
            .gotos
            .push_or_abort(GotoRef {
                name,
                jmp_pc: jmp,
                line,
                nactive,
            });
        Ok(())
    }
}
