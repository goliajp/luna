//! When hooks may run again after one failed (PUC `allowhook`).

use super::*;

impl Vm {
    /// A hook called at frame count `frames` is over: hooks are back on
    /// when it returned; an error out of it leaves them off (PUC
    /// `luaD_hook` sets `allowhook` again only after the hook returns)
    /// until a protected call below catches the error.
    pub(super) fn hook_left(&mut self, frames: usize, returned: bool) {
        if returned {
            self.in_hook = false;
        } else {
            self.hook_failed_at = frames;
        }
    }

    /// An error is caught, or leaves a host's call, with `frames` frames
    /// left: below a hook that failed, the hooks are on again (PUC
    /// `luaD_pcall` restores the `allowhook` it was entered with).
    #[inline(always)]
    pub(crate) fn hooks_caught(&mut self, frames: usize) {
        if self.in_hook && frames < self.hook_failed_at {
            self.in_hook = false;
            self.hook_failed_at = 0;
        }
    }
}
