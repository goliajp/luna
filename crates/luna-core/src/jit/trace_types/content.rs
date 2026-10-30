//! The content of a function prototype, as compiled code depends on it:
//! two prototypes of equal content compile to the same traces, whichever
//! load made them. The key a Vm finds earlier loads' traces by, and the
//! one it remembers failed trace heads by.

use crate::runtime::Value;
use crate::runtime::function::Proto;

/// What compiled code reads of a function prototype and of the prototypes
/// it creates closures of: its code, constants, upvalue layout and frame
/// shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtoContent {
    /// The serialized content, compared whole.
    pub bytes: Box<[u8]>,
    /// [`hash64`] of `bytes`: the key content is looked up by.
    pub hash: u64,
}

impl ProtoContent {
    /// The content of `p`.
    pub fn of(p: &Proto) -> ProtoContent {
        Self::of_ref(p).clone()
    }

    /// The content of `p`, made once and kept with it.
    pub fn of_ref(p: &Proto) -> &ProtoContent {
        p.content.get_or_init(|| Self::make(p))
    }

    fn make(p: &Proto) -> ProtoContent {
        let mut out = Vec::with_capacity(p.code.len() * 4 + 64);
        write_content(p, &mut out);
        let hash = hash64(&out);
        ProtoContent {
            bytes: out.into(),
            hash,
        }
    }

    /// Whether `p` has this content.
    pub fn matches(&self, p: &Proto) -> bool {
        // the cheap fields first: most prototypes differ in them
        if p.code.len() * 4 > self.bytes.len() || p.max_stack != self.bytes[self.bytes.len() - 1] {
            return false;
        }
        let mut out = Vec::with_capacity(self.bytes.len());
        write_content(p, &mut out);
        *out == *self.bytes
    }
}

fn write_content(p: &Proto, out: &mut Vec<u8>) {
    for i in p.code.iter() {
        out.extend_from_slice(&i.0.to_le_bytes());
    }
    out.extend_from_slice(&(p.consts.len() as u32).to_le_bytes());
    for c in p.consts.iter() {
        match c {
            Value::Nil => out.push(0),
            Value::Bool(b) => out.extend_from_slice(&[1, u8::from(*b)]),
            Value::Int(i) => {
                out.push(2);
                out.extend_from_slice(&i.to_le_bytes());
            }
            Value::Float(f) => {
                out.push(3);
                out.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            Value::Str(s) => {
                out.push(4);
                out.extend_from_slice(&(s.as_bytes().len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            // the compiler makes no other constants
            _ => out.push(255),
        }
    }
    write_upvals(p, out);
    // what creating a closure of a nested function reads of it
    out.extend_from_slice(&(p.protos.len() as u32).to_le_bytes());
    for c in p.protos.iter() {
        write_upvals(c, out);
        out.extend_from_slice(&[c.num_params, u8::from(c.is_vararg), c.max_stack]);
    }
    out.extend_from_slice(&[
        u8::from(p.has_vararg_table_pseudo),
        u8::from(p.has_compat_vararg_arg),
        p.env_upval_idx,
        p.num_params,
        u8::from(p.is_vararg),
        p.max_stack,
    ]);
}

fn write_upvals(p: &Proto, out: &mut Vec<u8>) {
    out.extend_from_slice(&(p.upvals.len() as u32).to_le_bytes());
    for u in p.upvals.iter() {
        let name = u.name.as_bytes();
        out.extend_from_slice(&[u8::from(u.in_stack), u.index, u8::from(u.read_only)]);
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name);
    }
}

/// A 64-bit hash of `b`, eight bytes at a time, for the bucket only.
pub fn hash64(b: &[u8]) -> u64 {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15 ^ b.len() as u64;
    let (chunks, rest) = b.as_chunks::<8>();
    for c in chunks {
        h = (h.rotate_left(5) ^ u64::from_le_bytes(*c)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    for &x in rest {
        h = (h.rotate_left(5) ^ u64::from(x)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    h ^ (h >> 29)
}
