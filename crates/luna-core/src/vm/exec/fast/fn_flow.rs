//! Where an arm run out of line goes: the loop's terminators as returns of
//! a [`Flow`](super::frame::Flow), which `run_fast` takes up where the arm
//! would have gone. The generator defines them inside each arm's function
//! (see `arm_fns`), where `leave!` writes the loop's locals back first.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_fn_flow_macros {
    ($d:tt) => {
        macro_rules! next {
            () => {
                leave!(Flow::Next)
            };
        }
        macro_rules! next_jumped {
            () => {
                leave!(Flow::NextJumped)
            };
        }
        macro_rules! resume {
            () => {
                leave!(Flow::Resume)
            };
        }
        macro_rules! resume_same {
            () => {
                leave!(Flow::ResumeSame)
            };
        }
        macro_rules! reenter {
            () => {
                leave!(Flow::Reenter)
            };
        }
        macro_rules! returned {
            ($d done:expr) => {
                leave!(Flow::Returned($d done))
            };
        }
        macro_rules! frames {
            () => {
                leave!(Flow::Frames)
            };
        }
        macro_rules! exit {
            ($d e:expr) => {
                leave!(Flow::Exit($d e))
            };
        }
    };
}
pub(super) use fast_fn_flow_macros;
