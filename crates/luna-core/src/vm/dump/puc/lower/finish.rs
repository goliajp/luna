//! Jump resolution and debug-table remapping that close out a [`Lowering`].

use super::{Jump, Lowered, Lowering, RawLocVar, enc_abx, enc_sj};
use crate::runtime::function::LocVar;
use crate::vm::isa;

impl Lowering {
    /// Resolve every jump and remap the debug tables.
    pub(in crate::vm::dump::puc) fn finish(
        mut self,
        raw_locvars: &[RawLocVar],
    ) -> Result<Lowered, String> {
        for f in std::mem::take(&mut self.fixups) {
            let Some(t) = self.first[f.target] else {
                self.pc = f.target;
                return Err(self.err("jump lands on an instruction that has no luna form"));
            };
            let (t, at) = (t as i64, f.at as i64);
            let inst = &mut self.code[f.at];
            let op = inst.op();
            let a = inst.a();
            *inst = match f.kind {
                Jump::Jmp => {
                    let sj = t - (at + 1);
                    if !(-(isa::MAX_SJ as i64)..=isa::MAX_SJ as i64).contains(&sj) {
                        return Err(format!(
                            "{} chunk: jump distance {sj} too long",
                            self.dialect
                        ));
                    }
                    enc_sj(op, sj as i32)?
                }
                Jump::ForPrep => enc_abx(op, a, bx_distance(self.dialect, t - at)?)?,
                Jump::Back => enc_abx(op, a, bx_distance(self.dialect, at + 1 - t)?)?,
                Jump::TForPrep => enc_abx(op, a, bx_distance(self.dialect, t - (at + 1))?)?,
                // PUC's 18-bit `sBx` reaches no further than the two
                // directions of `JmpClose` do
                Jump::JmpClose => isa::Inst::jmp_close(a, (t - (at + 1)) as i32),
            };
        }
        let locvars = self.locvars(raw_locvars)?;
        let max_stack = self.temp_base + self.temps_used;
        if max_stack > u8::MAX as u32 {
            return Err(format!(
                "{} chunk: translated frame needs {max_stack} registers, luna allows 255",
                self.dialect
            ));
        }
        Ok(Lowered {
            code: self.code,
            lines: self.lines,
            locvars,
            max_stack: max_stack as u8,
        })
    }

    /// First luna pc at or after PUC pc `pc` (the code length past the end).
    fn luna_pc(&self, pc: u32) -> u32 {
        self.first
            .iter()
            .skip(pc as usize)
            .find_map(|p| *p)
            .unwrap_or(self.code.len() as u32)
    }

    /// PUC dumps no register for a local: it is the number of locals still
    /// active when it was declared, since PUC gives locals consecutive
    /// registers in declaration order and `locvars` is in that order.
    fn locvars(&self, raw: &[RawLocVar]) -> Result<Vec<LocVar>, String> {
        let mut out = Vec::with_capacity(raw.len());
        for (i, v) in raw.iter().enumerate() {
            let puc_reg = raw[..i]
                .iter()
                .filter(|o| o.start_pc <= v.start_pc && v.start_pc < o.end_pc)
                .count() as u32;
            out.push(LocVar {
                name: v.name.clone(),
                reg: self.r(puc_reg)?,
                start_pc: self.luna_pc(v.start_pc),
                end_pc: self.luna_pc(v.end_pc),
            });
        }
        Ok(out)
    }
}

fn bx_distance(dialect: &str, d: i64) -> Result<u32, String> {
    if !(0..=isa::MAX_BX as i64).contains(&d) {
        return Err(format!(
            "{dialect} chunk: loop jump distance {d} out of luna's range"
        ));
    }
    Ok(d as u32)
}
