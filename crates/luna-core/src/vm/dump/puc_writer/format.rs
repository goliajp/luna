//! Chunk layout of each PUC version, as its `ldump.c` writes it: 5.1–5.3
//! with fixed-size integers, 5.4 and 5.5 with varints (of opposite
//! conventions), 5.5 with each string saved once and 4-byte alignment of
//! the code and absolute line info.

use super::{Dialect, Out};
use crate::runtime::Value;
use crate::vm::dump::puc::{puc_51, puc_52, puc_53, puc_54, puc_55};
use std::collections::HashMap;

pub(super) fn write(d: Dialect, root: &Out, strip: bool) -> Vec<u8> {
    let mut w = W {
        out: Vec::new(),
        strip,
        saved: HashMap::new(),
    };
    match d {
        Dialect::V51 => {
            w.out.extend_from_slice(puc_51::HEADER);
            w.f51(root, None);
        }
        Dialect::V52 => {
            w.out.extend_from_slice(puc_52::HEADER);
            w.f52(root);
        }
        Dialect::V53 => {
            w.out.extend_from_slice(puc_53::HEADER);
            w.out.push(root.upvals.len() as u8);
            w.f53(root, None);
        }
        Dialect::V54 => {
            w.out.extend_from_slice(puc_54::HEADER);
            w.out.push(root.upvals.len() as u8);
            w.f54(root, None);
        }
        Dialect::V55 => {
            w.out.extend_from_slice(puc_55::HEADER);
            w.out.push(root.upvals.len() as u8);
            w.f55(root);
        }
    }
    w.out
}

struct W {
    out: Vec<u8>,
    strip: bool,
    /// 5.5: strings written so far, by content, with their 1-based index
    saved: HashMap<Vec<u8>, u64>,
}

/// The source a nested function writes: none when it is its parent's (the
/// loader inherits it) or when stripping.
fn own_source<'a>(p: &'a Out, parent: Option<&[u8]>, strip: bool) -> Option<&'a [u8]> {
    (!strip && parent != Some(&p.source[..])).then_some(&p.source[..])
}

impl W {
    fn byte(&mut self, b: u8) {
        self.out.push(b);
    }

    fn int(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn code(&mut self, code: &[u32]) {
        for &i in code {
            self.int(i);
        }
    }

    /// Debug tables are empty when stripping.
    fn debug_len<T>(&self, v: &[T]) -> usize {
        if self.strip { 0 } else { v.len() }
    }

    // ---- 5.1 / 5.2: `int` and `size_t`-length strings with their NUL ----

    fn str51(&mut self, s: Option<&[u8]>) {
        match s {
            None => self.out.extend_from_slice(&0u64.to_le_bytes()),
            Some(s) => {
                self.out
                    .extend_from_slice(&(s.len() as u64 + 1).to_le_bytes());
                self.out.extend_from_slice(s);
                self.byte(0);
            }
        }
    }

    fn consts51(&mut self, consts: &[Value]) {
        self.int(consts.len() as u32);
        for v in consts {
            match *v {
                Value::Nil => self.byte(0),
                Value::Bool(b) => {
                    self.byte(1);
                    self.byte(b as u8);
                }
                Value::Float(f) => {
                    self.byte(3);
                    self.out.extend_from_slice(&f.to_le_bytes());
                }
                Value::Str(s) => {
                    self.byte(4);
                    self.str51(Some(s.as_bytes()));
                }
                // `consts_for` turned integers into floats
                _ => unreachable!("5.1/5.2 constant {}", v.type_name()),
            }
        }
    }

    fn locals51(&mut self, p: &Out) {
        let n = self.debug_len(&p.lines);
        self.int(n as u32);
        for &l in &p.lines[..n] {
            self.int(l);
        }
        let n = self.debug_len(&p.locvars);
        self.int(n as u32);
        for (name, start, end) in &p.locvars[..n] {
            self.str51(Some(name.as_bytes()));
            self.int(*start);
            self.int(*end);
        }
        let n = self.debug_len(&p.upvals);
        self.int(n as u32);
        for u in &p.upvals[..n] {
            self.str51(Some(u.name.as_bytes()));
        }
    }

    fn f51(&mut self, p: &Out, parent: Option<&[u8]>) {
        self.str51(own_source(p, parent, self.strip));
        self.int(p.line_defined);
        self.int(p.last_line_defined);
        self.byte(p.upvals.len() as u8);
        self.byte(p.num_params);
        self.byte(p.vararg);
        self.byte(p.max_stack);
        self.int(p.code.len() as u32);
        self.code(&p.code);
        self.consts51(&p.consts);
        self.int(p.protos.len() as u32);
        for c in &p.protos {
            self.f51(c, Some(&p.source));
        }
        self.locals51(p);
    }

    fn f52(&mut self, p: &Out) {
        self.int(p.line_defined);
        self.int(p.last_line_defined);
        self.byte(p.num_params);
        self.byte(p.vararg);
        self.byte(p.max_stack);
        self.int(p.code.len() as u32);
        self.code(&p.code);
        self.consts51(&p.consts);
        self.int(p.protos.len() as u32);
        for c in &p.protos {
            self.f52(c);
        }
        self.int(p.upvals.len() as u32);
        for u in &p.upvals {
            self.byte(u.in_stack as u8);
            self.byte(u.index);
        }
        let source = (!self.strip).then_some(&p.source[..]);
        self.str51(source);
        self.locals51(p);
    }

    // ---- 5.3: byte-length strings without their NUL ----

    fn str53(&mut self, s: Option<&[u8]>) {
        let Some(s) = s else {
            return self.byte(0);
        };
        let size = s.len() as u64 + 1;
        if size < 0xFF {
            self.byte(size as u8);
        } else {
            self.byte(0xFF);
            self.out.extend_from_slice(&size.to_le_bytes());
        }
        self.out.extend_from_slice(s);
    }

    fn f53(&mut self, p: &Out, parent: Option<&[u8]>) {
        self.str53(own_source(p, parent, self.strip));
        self.int(p.line_defined);
        self.int(p.last_line_defined);
        self.byte(p.num_params);
        self.byte(p.vararg);
        self.byte(p.max_stack);
        self.int(p.code.len() as u32);
        self.code(&p.code);
        self.int(p.consts.len() as u32);
        for v in &p.consts {
            match *v {
                Value::Nil => self.byte(0),
                Value::Bool(b) => {
                    self.byte(1);
                    self.byte(b as u8);
                }
                Value::Float(f) => {
                    self.byte(3);
                    self.out.extend_from_slice(&f.to_le_bytes());
                }
                Value::Int(i) => {
                    self.byte(19);
                    self.out.extend_from_slice(&i.to_le_bytes());
                }
                Value::Str(s) => {
                    self.byte(if s.len() <= 40 { 4 } else { 20 });
                    self.str53(Some(s.as_bytes()));
                }
                _ => unreachable!("constant {}", v.type_name()),
            }
        }
        self.int(p.upvals.len() as u32);
        for u in &p.upvals {
            self.byte(u.in_stack as u8);
            self.byte(u.index);
        }
        self.int(p.protos.len() as u32);
        for c in &p.protos {
            self.f53(c, Some(&p.source));
        }
        let n = self.debug_len(&p.lines);
        self.int(n as u32);
        for &l in &p.lines[..n] {
            self.int(l);
        }
        let n = self.debug_len(&p.locvars);
        self.int(n as u32);
        for (name, start, end) in &p.locvars[..n] {
            self.str53(Some(name.as_bytes()));
            self.int(*start);
            self.int(*end);
        }
        let n = self.debug_len(&p.upvals);
        self.int(n as u32);
        for u in &p.upvals[..n] {
            self.str53(Some(u.name.as_bytes()));
        }
    }

    // ---- 5.4: varints whose last byte has the high bit set ----

    fn var54(&mut self, mut x: u64) {
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
        self.out.extend_from_slice(&buf[10 - n..]);
    }

    fn str54(&mut self, s: Option<&[u8]>) {
        match s {
            None => self.var54(0),
            Some(s) => {
                self.var54(s.len() as u64 + 1);
                self.out.extend_from_slice(s);
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

    fn f54(&mut self, p: &Out, parent: Option<&[u8]>) {
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
                Value::Int(i) => self.out.extend_from_slice(&i.to_le_bytes()),
                Value::Float(f) => self.out.extend_from_slice(&f.to_le_bytes()),
                Value::Str(s) => self.str54(Some(s.as_bytes())),
                _ => {}
            }
        }
        self.var54(p.upvals.len() as u64);
        for u in &p.upvals {
            self.out
                .extend_from_slice(&[u.in_stack as u8, u.index, u.kind]);
        }
        self.var54(p.protos.len() as u64);
        for c in &p.protos {
            self.f54(c, Some(&p.source));
        }
        let (deltas, abs) = self.line_info(p);
        self.var54(deltas.len() as u64);
        self.out.extend_from_slice(&deltas);
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

    fn var55(&mut self, mut x: u64) {
        let mut buf = [0u8; 10];
        let mut n = 1;
        buf[9] = (x & 0x7F) as u8;
        x >>= 7;
        while x != 0 {
            n += 1;
            buf[10 - n] = (x & 0x7F) as u8 | 0x80;
            x >>= 7;
        }
        self.out.extend_from_slice(&buf[10 - n..]);
    }

    fn align4(&mut self) {
        while !self.out.len().is_multiple_of(4) {
            self.byte(0);
        }
    }

    fn str55(&mut self, s: Option<&[u8]>) {
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

    fn f55(&mut self, p: &Out) {
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
                Value::Float(f) => self.out.extend_from_slice(&f.to_le_bytes()),
                Value::Str(s) => self.str55(Some(s.as_bytes())),
                _ => {}
            }
        }
        self.var55(p.upvals.len() as u64);
        for u in &p.upvals {
            self.out
                .extend_from_slice(&[u.in_stack as u8, u.index, u.kind]);
        }
        self.var55(p.protos.len() as u64);
        for c in &p.protos {
            self.f55(c);
        }
        let source = (!self.strip).then_some(&p.source[..]);
        self.str55(source);
        let (deltas, abs) = self.line_info(p);
        self.var55(deltas.len() as u64);
        self.out.extend_from_slice(&deltas);
        self.var55(abs.len() as u64);
        if !abs.is_empty() {
            self.align4();
            for &(pc, line) in &abs {
                self.int(pc);
                self.int(line);
            }
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
fn line_info(p: &Out) -> (Vec<u8>, Vec<(u32, u32)>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn w() -> W {
        W {
            out: Vec::new(),
            strip: false,
            saved: HashMap::new(),
        }
    }

    #[test]
    fn varints_use_each_versions_end_marker() {
        let mut a = w();
        a.var54(300);
        // 5.4: most significant group first, last byte flagged
        assert_eq!(a.out, [0x02, 0x2C | 0x80]);
        let mut b = w();
        b.var55(300);
        // 5.5: every byte but the last flagged
        assert_eq!(b.out, [0x02 | 0x80, 0x2C]);
    }

    #[test]
    fn a_repeated_string_is_a_back_reference_in_5_5() {
        let mut a = w();
        a.str55(Some(b"ab"));
        a.str55(Some(b"ab"));
        a.str55(None);
        assert_eq!(a.out, [3, b'a', b'b', 0, 0, 1, 0, 0]);
    }

    fn out_with_lines(line_defined: u32, lines: Vec<u32>) -> Out {
        Out {
            source: Vec::new(),
            line_defined,
            last_line_defined: 0,
            num_params: 0,
            vararg: 0,
            max_stack: 2,
            code: vec![0; lines.len()],
            lines,
            consts: Vec::new(),
            upvals: Vec::new(),
            protos: Vec::new(),
            locvars: Vec::new(),
        }
    }

    #[test]
    fn line_info_goes_absolute_on_a_far_jump_and_every_129th_entry() {
        let (d, abs) = line_info(&out_with_lines(10, vec![11, 500, 499]));
        assert_eq!(d, [1, 0x80, 0xFF]);
        assert_eq!(abs, [(1, 500)]);
        let (d, abs) = line_info(&out_with_lines(1, vec![1; 300]));
        assert_eq!(abs, [(128, 1), (256, 1)]);
        assert_eq!(d.iter().filter(|&&x| x == 0x80).count(), 2);
    }
}
