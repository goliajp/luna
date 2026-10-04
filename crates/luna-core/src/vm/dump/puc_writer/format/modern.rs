//! PUC 5.4 / 5.5 layouts: varints (of opposite conventions), 5.5 string reuse and alignment.

use super::{W, own_source};
use crate::runtime::Value;
use crate::vm::dump::puc_writer::Out;

impl W {
    // ---- 5.4: varints whose last byte has the high bit set ----

    pub(super) fn var54(&mut self, mut x: u64) {
        let mut buf = [0u8; 10];
        let mut n = 0;
        loop {
            buf[9 - n] = (x & 0x7F) as u8;
            n += 1;
            x >>= 7;
            if x == 0 {
                break;
            }
        }
        buf[9] |= 0x80;
        self.bytes(&buf[10 - n..]);
    }

    fn str54(&mut self, s: Option<&[u8]>) {
        match s {
            None => self.var54(0),
            Some(s) => {
                self.var54(s.len() as u64 + 1);
                self.bytes(s);
            }
        }
    }

    /// Constant tag and payload shared by 5.4 and 5.5 (`makevariant`).
    fn const_tag(&mut self, v: &Value) {
        let tag = match v {
            Value::Nil => 0,
            Value::Bool(false) => 1,
            Value::Bool(true) => 17,
            Value::Int(_) => 3,
            Value::Float(_) => 19,
            Value::Str(s) if s.len() <= 40 => 4,
            Value::Str(_) => 20,
            _ => unreachable!("constant {}", v.type_name()),
        };
        self.byte(tag);
    }

    pub(super) fn f54(&mut self, p: &Out, parent: Option<&[u8]>) {
        self.str54(own_source(p, parent, self.strip));
        self.var54(p.line_defined as u64);
        self.var54(p.last_line_defined as u64);
        self.byte(p.num_params);
        self.byte(p.vararg);
        self.byte(p.max_stack);
        self.var54(p.code.len() as u64);
        self.code(&p.code);
        self.var54(p.consts.len() as u64);
        for v in &p.consts {
            self.const_tag(v);
            match *v {
                Value::Int(i) => self.bytes(&i.to_le_bytes()),
                Value::Float(f) => self.bytes(&f.to_le_bytes()),
                Value::Str(s) => self.str54(Some(s.as_bytes())),
                _ => {}
            }
        }
        self.var54(p.upvals.len() as u64);
        for u in &p.upvals {
            self.byte(u.in_stack as u8);
            self.byte(u.index);
            self.byte(u.kind);
        }
        self.var54(p.protos.len() as u64);
        for c in &p.protos {
            self.f54(c, Some(&p.source));
        }
        let (deltas, abs) = self.line_info(p);
        self.var54(deltas.len() as u64);
        self.bytes(&deltas);
        self.var54(abs.len() as u64);
        for &(pc, line) in &abs {
            self.var54(pc as u64);
            self.var54(line as u64);
        }
        let n = self.debug_len(&p.locvars);
        self.var54(n as u64);
        for (name, start, end) in &p.locvars[..n] {
            self.str54(Some(name.as_bytes()));
            self.var54(*start as u64);
            self.var54(*end as u64);
        }
        let n = self.debug_len(&p.upvals);
        self.var54(n as u64);
        for u in &p.upvals[..n] {
            self.str54(Some(u.name.as_bytes()));
        }
    }

    // ---- 5.5: varints with a continuation bit; strings saved once ----

    pub(super) fn var55(&mut self, mut x: u64) {
        let mut buf = [0u8; 10];
        let mut n = 1;
        buf[9] = (x & 0x7F) as u8;
        x >>= 7;
        while x != 0 {
            n += 1;
            buf[10 - n] = (x & 0x7F) as u8 | 0x80;
            x >>= 7;
        }
        self.bytes(&buf[10 - n..]);
    }

    /// Zeros up to a multiple of 4 bytes, as one block when any are needed.
    fn align4(&mut self) {
        if self.out.len().is_multiple_of(4) {
            return;
        }
        while !self.out.len().is_multiple_of(4) {
            self.out.push(0);
        }
        self.cut();
    }

    pub(super) fn str55(&mut self, s: Option<&[u8]>) {
        let Some(s) = s else {
            self.var55(0);
            return self.var55(0);
        };
        if let Some(&idx) = self.saved.get(s) {
            self.var55(0);
            return self.var55(idx);
        }
        self.var55(s.len() as u64 + 1);
        self.out.extend_from_slice(s);
        self.byte(0);
        let idx = self.saved.len() as u64 + 1;
        self.saved.insert(s.to_vec(), idx);
    }

    pub(super) fn f55(&mut self, p: &Out) {
        self.var55(p.line_defined as u64);
        self.var55(p.last_line_defined as u64);
        self.byte(p.num_params);
        self.byte(p.vararg);
        self.byte(p.max_stack);
        self.var55(p.code.len() as u64);
        self.align4();
        self.code(&p.code);
        self.var55(p.consts.len() as u64);
        for v in &p.consts {
            self.const_tag(v);
            match *v {
                // zig-zag: 2x for x >= 0, -2x - 1 below
                Value::Int(i) => self.var55(((i << 1) ^ (i >> 63)) as u64),
                Value::Float(f) => self.bytes(&f.to_le_bytes()),
                Value::Str(s) => self.str55(Some(s.as_bytes())),
                _ => {}
            }
        }
        self.var55(p.upvals.len() as u64);
        for u in &p.upvals {
            self.byte(u.in_stack as u8);
            self.byte(u.index);
            self.byte(u.kind);
        }
        self.var55(p.protos.len() as u64);
        for c in &p.protos {
            self.f55(c);
        }
        let source = (!self.strip).then_some(&p.source[..]);
        self.str55(source);
        let (deltas, abs) = self.line_info(p);
        self.var55(deltas.len() as u64);
        self.bytes(&deltas);
        self.var55(abs.len() as u64);
        if !abs.is_empty() {
            self.align4();
            let pairs: Vec<u32> = abs.iter().flat_map(|&(pc, line)| [pc, line]).collect();
            self.ints(&pairs);
        }
        let n = self.debug_len(&p.locvars);
        self.var55(n as u64);
        for (name, start, end) in &p.locvars[..n] {
            self.str55(Some(name.as_bytes()));
            self.var55(*start as u64);
            self.var55(*end as u64);
        }
        let n = self.debug_len(&p.upvals);
        self.var55(n as u64);
        for u in &p.upvals[..n] {
            self.str55(Some(u.name.as_bytes()));
        }
    }
}

impl W {
    fn line_info(&self, p: &Out) -> (Vec<u8>, Vec<(u32, u32)>) {
        if self.strip {
            return (Vec::new(), Vec::new());
        }
        line_info(p)
    }
}

/// `lcode.c` `savelineinfo`: a signed byte delta per instruction from the
/// previous one's line (the function's first line to start), with an
/// absolute entry instead when the delta does not fit a byte or after
/// every 128 relative ones (`MAXIWTHABS`), which `getbaseline` relies on.
pub(super) fn line_info(p: &Out) -> (Vec<u8>, Vec<(u32, u32)>) {
    let mut deltas = Vec::with_capacity(p.lines.len());
    let mut abs = Vec::new();
    let mut prev = p.line_defined as i64;
    let mut since_abs = 0;
    for (pc, &line) in p.lines.iter().enumerate() {
        let d = line as i64 - prev;
        since_abs += 1;
        if d.abs() >= 0x80 || since_abs > 128 {
            abs.push((pc as u32, line));
            deltas.push(0x80);
            since_abs = 1;
        } else {
            deltas.push(d as i8 as u8);
        }
        prev = line as i64;
    }
    (deltas, abs)
}
