use super::modern::line_info;
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
