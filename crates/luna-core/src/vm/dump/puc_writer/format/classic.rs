//! PUC 5.1–5.3 layouts: fixed-size integers and length-prefixed strings.

use super::{W, own_source};
use crate::runtime::Value;
use crate::vm::dump::puc_writer::Out;

impl W {
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

    pub(super) fn f51(&mut self, p: &Out, parent: Option<&[u8]>) {
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

    pub(super) fn f52(&mut self, p: &Out) {
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

    pub(super) fn f53(&mut self, p: &Out, parent: Option<&[u8]>) {
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
}
