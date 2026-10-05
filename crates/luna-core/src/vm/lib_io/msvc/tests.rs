use std::io::{Cursor, SeekFrom};

use super::*;

/// A file in memory.
struct Mem(Cursor<Vec<u8>>);

impl Os for Mem {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.0, buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut self.0, buf)
    }
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut self.0, from)
    }
    fn len(&mut self) -> std::io::Result<u64> {
        Ok(self.0.get_ref().len() as u64)
    }
}

fn mem(bytes: &[u8]) -> Mem {
    Mem(Cursor::new(bytes.to_vec()))
}

/// `f:read(1)`, `f:seek("cur")`, `f:read("l")`, `f:seek("cur")` on 300
/// lines `abc\n` read in text mode after `setvbuf`, as PUC 5.4 built with
/// MSVC printed them (the second position is one past the true one).
#[test]
fn positions_after_setvbuf_match_the_library() {
    for (mode, size, want) in [(2, 0, [1, 4]), (0, 100, [1, 5]), (1, 30, [1, 5])] {
        let mut os = mem(&b"abc\n".repeat(300));
        let mut f = CrtFile::open(b"r", true, false);
        f.setvbuf(&mut os, mode, size);
        assert_eq!(f.fread(&mut os, 1), b"a");
        assert!(f.fseek(&mut os, 0, 1));
        assert_eq!(f.ftell(&mut os), want[0]);
        let mut line = Vec::new();
        while let Some(c) = f.getc(&mut os) {
            if c == b'\n' {
                break;
            }
            line.push(c);
        }
        assert_eq!(line, b"bc");
        assert!(f.fseek(&mut os, 0, 1));
        assert_eq!(f.ftell(&mut os), want[1], "mode {mode} size {size}");
    }
}

#[test]
fn text_mode_translates_both_ways() {
    let mut os = mem(b"");
    let mut f = CrtFile::open(b"w", true, false);
    assert_eq!(f.fwrite(&mut os, b"a\nb\r\n"), 5);
    assert!(f.fflush(&mut os));
    assert_eq!(os.0.get_ref(), b"a\r\nb\r\r\n");
    let mut os = mem(b"a\r\nb\r\r\nc\x1ad");
    let mut f = CrtFile::open(b"r", true, false);
    assert_eq!(f.fread(&mut os, 100), b"a\nb\r\nc");
    assert_eq!(translate_all(b"a\r\nb\x1ac"), b"a\nb");
}

/// A write after a read that stopped short of the end writes nothing (and
/// sets no error) until a seek.
#[test]
fn write_after_read_fails_until_a_seek() {
    let mut os = mem(b"aa\nbb\n");
    let mut f = CrtFile::open(b"r+", false, false);
    assert_eq!(f.getc(&mut os), Some(b'a'));
    assert_eq!(f.fwrite(&mut os, b"X"), 0);
    assert!(!f.ferror());
    assert!(f.fseek(&mut os, 0, 1));
    assert_eq!(f.fwrite(&mut os, b"X"), 1);
}

/// `ungetc` with the pointer at the start of a buffer that still holds
/// bytes is refused.
#[test]
fn ungetc_at_the_start_of_a_full_buffer_is_refused() {
    let mut os = mem(b"12345");
    let mut f = CrtFile::open(b"r", false, false);
    assert_eq!(f.getc(&mut os), Some(b'1'));
    assert!(f.ungetc(b'1'));
    assert!(!f.ungetc(b'0'));
    assert_eq!(f.fread(&mut os, 10), b"12345");
}

#[test]
fn scanf_takes_what_msvc_takes() {
    for (input, want, rest) in [
        (&b"1e+x"[..], None, &b"x"[..]),
        (b"  12abc", Some(12.0), b"abc"),
        (b"1..2", Some(1.0), b".2"),
        (b"-.e1", None, b"e1"),
        (b"0x", None, b""),
        (b"inf", Some(f64::INFINITY), b""),
        (b"0x1P-2z", Some(0.25), b"z"),
        (b"--1", None, b"-1"),
        (b"0x.8", Some(0.5), b""),
        (b"- 3", None, b" 3"),
    ] {
        let mut os = mem(input);
        let mut f = CrtFile::open(b"r", false, false);
        assert_eq!(scan::scan_double(&mut f, &mut os), want, "{input:?}");
        assert_eq!(f.fread(&mut os, 100), rest, "{input:?}");
    }
}
