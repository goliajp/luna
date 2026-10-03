//! Parallel moves: block arguments into parameters and call arguments into
//! argument registers, where a destination may be another move's source.

use super::alloc::Loc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Src {
    Loc(Loc),
}

/// The moves of one register class, in an order that reads every source
/// before it is overwritten; `park` is a location no move touches, used to
/// break cycles.
pub(crate) fn sequence(moves: &mut Vec<(Loc, Src)>, park: Loc, out: &mut Vec<(Loc, Src)>) {
    moves.retain(|&(d, s)| s != Src::Loc(d) && d != Loc::None);
    out.clear();
    while !moves.is_empty() {
        let ready = (0..moves.len()).find(|&k| {
            let d = moves[k].0;
            !moves
                .iter()
                .enumerate()
                .any(|(j, &(_, s))| j != k && s == Src::Loc(d))
        });
        match ready {
            Some(k) => out.push(moves.swap_remove(k)),
            None => {
                // every destination is still read: a cycle; park one source
                let s = moves[0].1;
                out.push((park, s));
                for m in moves.iter_mut() {
                    if m.1 == s {
                        m.1 = Src::Loc(park);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(moves: &[(Loc, Src)]) -> std::collections::HashMap<Loc, i64> {
        // initial state: register r holds r * 10, slot s holds 1000 + s
        let init = |l: Loc| match l {
            Loc::Reg(r) => i64::from(r) * 10,
            Loc::Stack(s) => 1000 + i64::from(s),
            Loc::None => -1,
        };
        let mut state = std::collections::HashMap::new();
        let read = |st: &std::collections::HashMap<Loc, i64>, l: Loc| {
            st.get(&l).copied().unwrap_or_else(|| init(l))
        };
        let mut v = moves.to_vec();
        let mut out = Vec::new();
        sequence(&mut v, Loc::Reg(99), &mut out);
        for (d, s) in out {
            let Src::Loc(l) = s;
            let x = read(&state, l);
            state.insert(d, x);
        }
        let want: Vec<(Loc, i64)> = moves
            .iter()
            .map(|&(d, s)| {
                let Src::Loc(l) = s;
                (d, init(l))
            })
            .collect();
        for (d, x) in want {
            assert_eq!(read(&state, d), x, "{d:?} in {moves:?}");
        }
        state
    }

    #[test]
    fn a_swap_goes_through_the_park_location() {
        run(&[
            (Loc::Reg(1), Src::Loc(Loc::Reg(2))),
            (Loc::Reg(2), Src::Loc(Loc::Reg(1))),
        ]);
    }

    #[test]
    fn a_rotation_and_a_chain_keep_every_source() {
        run(&[
            (Loc::Reg(1), Src::Loc(Loc::Reg(2))),
            (Loc::Reg(2), Src::Loc(Loc::Reg(3))),
            (Loc::Reg(3), Src::Loc(Loc::Reg(1))),
            (Loc::Reg(4), Src::Loc(Loc::Reg(3))),
            (Loc::Stack(0), Src::Loc(Loc::Reg(4))),
        ]);
    }

    #[test]
    fn one_source_fans_out() {
        run(&[
            (Loc::Reg(1), Src::Loc(Loc::Reg(2))),
            (Loc::Reg(3), Src::Loc(Loc::Reg(2))),
            (Loc::Reg(2), Src::Loc(Loc::Reg(1))),
        ]);
    }
}
