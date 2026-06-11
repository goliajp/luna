//! P02 decision bench: 16-byte tagged enum vs 8-byte NaN-box value
//! representation, plus enum-array vs split tag/payload arrays (the 5.5
//! "compact arrays" layout) for table storage.
//!
//! NaN-box caveat being measured: Lua 5.5 has native i64 integers; a NaN box
//! holds only 47-bit small ints, so every integer op pays a range check (and
//! overflow would need boxing — not even modeled here, so the NaN-box numbers
//! below are its *best case*).

use std::hint::black_box;
use std::time::Instant;

const N: usize = 1 << 20;
const ROUNDS: u32 = 64;

// ---- candidate A: 16-byte tagged enum ----

#[derive(Clone, Copy)]
enum V16 {
    Nil,
    Int(i64),
    Float(f64),
}

// ---- candidate B: 8-byte NaN box (negative qNaN space) ----

#[derive(Clone, Copy, PartialEq)]
struct V8(u64);

const QNAN: u64 = 0x7FF8_0000_0000_0000;
const TAG_MASK: u64 = 0xFFFF_8000_0000_0000;
const TAG_INT: u64 = 0xFFF8_8000_0000_0000; // qNaN | sign | tag bit
const TAG_NIL: u64 = 0xFFF9_0000_0000_0000;
const PAYLOAD47: u64 = 0x0000_7FFF_FFFF_FFFF;

impl V8 {
    #[inline(always)]
    fn float(x: f64) -> V8 {
        V8(x.to_bits())
    }
    #[inline(always)]
    fn int(x: i64) -> V8 {
        debug_assert!(fits47(x));
        V8(TAG_INT | (x as u64 & PAYLOAD47))
    }
    #[inline(always)]
    fn nil() -> V8 {
        V8(TAG_NIL)
    }
    #[inline(always)]
    fn is_int(self) -> bool {
        self.0 & TAG_MASK == TAG_INT
    }
    #[inline(always)]
    fn is_float(self) -> bool {
        // anything outside the boxed-tag space is a real double
        self.0 & QNAN != QNAN || self.0 & TAG_MASK == self.0 & 0xFFF0_0000_0000_0000
    }
    #[inline(always)]
    fn as_int(self) -> i64 {
        // sign-extend 47-bit payload
        ((self.0 & PAYLOAD47) as i64) << 17 >> 17
    }
    #[inline(always)]
    fn as_float(self) -> f64 {
        f64::from_bits(self.0)
    }
}

#[inline(always)]
fn fits47(x: i64) -> bool {
    (x << 17 >> 17) == x
}

// ---- candidate C (table arrays): split tag/payload ----

struct Split {
    tags: Vec<u8>,
    vals: Vec<u64>,
}

const ST_NIL: u8 = 0;
const ST_INT: u8 = 1;
const ST_FLOAT: u8 = 2;

fn bench(name: &str, mut f: impl FnMut() -> u64) {
    // warmup
    black_box(f());
    let t0 = Instant::now();
    let mut acc = 0u64;
    for _ in 0..ROUNDS {
        acc = acc.wrapping_add(black_box(f()));
    }
    let dt = t0.elapsed();
    let per = dt.as_nanos() as f64 / (ROUNDS as f64 * N as f64);
    println!("{name:34} {per:6.3} ns/elem   (check {acc:x})");
}

fn main() {
    // mixed workload: ~60% ints, ~30% floats, 10% nil; deterministic LCG
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed >> 33
    };
    let kinds: Vec<u8> = (0..N).map(|_| (rnd() % 10) as u8).collect();
    let ints: Vec<i64> = (0..N).map(|_| (rnd() as i64) % (1 << 40)).collect();

    let a16: Vec<V16> = (0..N)
        .map(|i| match kinds[i] {
            0..=5 => V16::Int(ints[i]),
            6..=8 => V16::Float(ints[i] as f64 * 0.5),
            _ => V16::Nil,
        })
        .collect();
    let b8: Vec<V8> = (0..N)
        .map(|i| match kinds[i] {
            0..=5 => V8::int(ints[i]),
            6..=8 => V8::float(ints[i] as f64 * 0.5),
            _ => V8::nil(),
        })
        .collect();
    let split = Split {
        tags: (0..N)
            .map(|i| match kinds[i] {
                0..=5 => ST_INT,
                6..=8 => ST_FLOAT,
                _ => ST_NIL,
            })
            .collect(),
        vals: (0..N)
            .map(|i| match kinds[i] {
                0..=5 => ints[i] as u64,
                6..=8 => (ints[i] as f64 * 0.5).to_bits(),
                _ => 0,
            })
            .collect(),
    };

    println!("-- workload 1: arithmetic dispatch (sum) --");
    bench("enum16 dispatch", || {
        let (mut ia, mut fa) = (0i64, 0f64);
        for v in &a16 {
            match *v {
                V16::Int(x) => ia = ia.wrapping_add(x),
                V16::Float(x) => fa += x,
                V16::Nil => {}
            }
        }
        ia as u64 ^ fa.to_bits()
    });
    bench("nanbox8 dispatch (+range check)", || {
        let (mut ia, mut fa) = (0i64, 0f64);
        for v in &b8 {
            if v.is_int() {
                let x = v.as_int();
                let s = ia.wrapping_add(x);
                // integer result must stay in smi range or box: model the check
                if fits47(s) {
                    ia = s;
                } else {
                    ia = s & PAYLOAD47 as i64;
                }
            } else if v.is_float() && *v != V8::nil() {
                fa += v.as_float();
            }
        }
        ia as u64 ^ fa.to_bits()
    });

    println!("-- workload 2: value move bandwidth (gather) --");
    let idx: Vec<u32> = (0..N).map(|_| (rnd() % N as u64) as u32).collect();
    let mut dst16 = vec![V16::Nil; N];
    bench("enum16 gather", || {
        for i in 0..N {
            dst16[i] = a16[idx[i] as usize];
        }
        unsafe { std::ptr::read(&dst16[N / 2] as *const V16 as *const u64) }
    });
    let mut dst8 = vec![V8::nil(); N];
    bench("nanbox8 gather", || {
        for i in 0..N {
            dst8[i] = b8[idx[i] as usize];
        }
        dst8[N / 2].0
    });

    println!("-- workload 3: array traversal (table array part) --");
    bench("Vec<enum16> scan", || {
        let mut h = 0u64;
        for v in &a16 {
            if let V16::Int(x) = *v {
                h = h.wrapping_add(x as u64);
            }
        }
        h
    });
    bench("split tags+payload scan", || {
        let mut h = 0u64;
        for i in 0..N {
            if split.tags[i] == ST_INT {
                h = h.wrapping_add(split.vals[i]);
            }
        }
        h
    });

    println!(
        "sizes: enum16={}B nanbox={}B split={}B/slot",
        size_of::<V16>(),
        size_of::<V8>(),
        size_of::<u8>() + size_of::<u64>()
    );
}
