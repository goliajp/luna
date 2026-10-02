//! Debug records of a compiled function: its upvalues and local variables.

/// Where a closure's upvalue is captured from, relative to the *enclosing*
/// function (PUC Upvaldesc).
#[derive(Clone, Debug)]
pub struct UpvalDesc {
    /// captured from the enclosing frame's registers (true) or from the
    /// enclosing closure's own upvalues (false)
    pub in_stack: bool,
    /// Index in the enclosing frame's register file (when `in_stack`) or
    /// in the enclosing closure's upvalue array (otherwise).
    pub index: u8,
    /// variable name, for error messages and debug info
    pub name: DebugName,
    /// the captured variable is `<const>` (5.5): assignment through this
    /// upvalue is a compile-time error
    pub read_only: bool,
}

/// Debug record for a local variable: its name and the pc range over which it
/// occupies register `reg`. Used to name registers in error messages and
/// debug.getinfo (PUC LocVar).
#[derive(Clone, Debug)]
pub struct LocVar {
    /// Local-variable name.
    pub name: DebugName,
    /// Register holding the variable while in scope.
    pub reg: u32,
    /// First pc where the variable is live.
    pub start_pc: u32,
    /// Pc one past the last where the variable is live.
    pub end_pc: u32,
}

/// The name of a local variable or upvalue in a function's debug records.
/// It reads as a `&str` (it derefs to one); a name of up to
/// [`DebugName::INLINE_LEN`] bytes is stored in place, so loading a chunk
/// does not allocate for each of its local names.
#[derive(Clone)]
pub struct DebugName(Repr);

#[derive(Clone)]
enum Repr {
    Inline {
        len: u8,
        bytes: [u8; DebugName::INLINE_LEN],
    },
    Heap(Box<str>),
}

impl DebugName {
    /// The longest name kept without a heap allocation.
    pub const INLINE_LEN: usize = 22;

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Inline { len, bytes } => {
                // SAFETY: the bytes were copied from a `&str` and cut at its
                // end
                unsafe { std::str::from_utf8_unchecked(&bytes[..*len as usize]) }
            }
            Repr::Heap(s) => s,
        }
    }
}

impl From<&str> for DebugName {
    fn from(s: &str) -> DebugName {
        if s.len() <= DebugName::INLINE_LEN {
            let mut bytes = [0; DebugName::INLINE_LEN];
            bytes[..s.len()].copy_from_slice(s.as_bytes());
            DebugName(Repr::Inline {
                len: s.len() as u8,
                bytes,
            })
        } else {
            DebugName(Repr::Heap(s.into()))
        }
    }
}

impl From<String> for DebugName {
    fn from(s: String) -> DebugName {
        if s.len() <= DebugName::INLINE_LEN {
            DebugName::from(s.as_str())
        } else {
            DebugName(Repr::Heap(s.into_boxed_str()))
        }
    }
}

impl From<Box<str>> for DebugName {
    fn from(s: Box<str>) -> DebugName {
        if s.len() <= DebugName::INLINE_LEN {
            DebugName::from(&*s)
        } else {
            DebugName(Repr::Heap(s))
        }
    }
}

impl From<std::borrow::Cow<'_, str>> for DebugName {
    fn from(s: std::borrow::Cow<'_, str>) -> DebugName {
        match s {
            std::borrow::Cow::Borrowed(s) => DebugName::from(s),
            std::borrow::Cow::Owned(s) => DebugName::from(s),
        }
    }
}

impl Default for DebugName {
    fn default() -> DebugName {
        DebugName::from("")
    }
}

impl std::ops::Deref for DebugName {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for DebugName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for DebugName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for DebugName {
    fn eq(&self, o: &DebugName) -> bool {
        self.as_str() == o.as_str()
    }
}

impl Eq for DebugName {}

impl PartialEq<str> for DebugName {
    fn eq(&self, o: &str) -> bool {
        self.as_str() == o
    }
}

impl PartialEq<&str> for DebugName {
    fn eq(&self, o: &&str) -> bool {
        self.as_str() == *o
    }
}

impl std::hash::Hash for DebugName {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.as_str().hash(h)
    }
}

impl std::fmt::Debug for DebugName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl std::fmt::Display for DebugName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.as_str(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::DebugName;

    #[test]
    fn short_and_long_names_read_back() {
        for s in [
            "",
            "x",
            "_ENV",
            "exactly_twenty_two_ch_",
            "a_name_longer_than_twenty_two_bytes",
            "名前",
        ] {
            let n = DebugName::from(s);
            assert_eq!(n.as_str(), s);
            assert_eq!(n, DebugName::from(s.to_string()));
            assert_eq!(&*n, s);
            assert_eq!(n.to_string(), s);
        }
        assert_eq!(std::mem::size_of::<DebugName>(), 24);
    }
}
