//! Reading the REPL's lines: stdin as `fgets` reads it, or the line
//! editor on a terminal.

use crate::Interp;

/// `LUA_MAXINPUT`: the size of `lua.c`'s line buffer.
const MAXINPUT: usize = 512;

/// A line as the reader gave it.
pub(crate) enum Line {
    Text(Vec<u8>),
    Eof,
    /// the line editor's Ctrl-C: drop the statement being typed
    #[cfg(feature = "repl-line-editor")]
    Cancel,
}

pub(super) enum Input {
    Stdin,
    #[cfg(feature = "repl-line-editor")]
    Editor(Box<crate::line_editor::LineEditor>),
}

impl Input {
    pub(super) fn new() -> Input {
        #[cfg(feature = "repl-line-editor")]
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            match crate::line_editor::LineEditor::new() {
                Ok(ed) => return Input::Editor(Box::new(ed)),
                Err(e) => eprintln!("line editor unavailable ({e}); reading plain lines"),
            }
        }
        Input::Stdin
    }

    /// `lua_readline`.
    pub(super) fn read(&mut self, interp: &mut Interp, prompt: &[u8]) -> Line {
        match self {
            Input::Stdin => {
                // unchecked, as lua.c's fputs / fflush
                luna_core::stdio::write_stdout(prompt);
                let _ = luna_core::stdio::flush_stdout();
                match interp.vm.read_stdin_line(MAXINPUT) {
                    Ok(Some(l)) => Line::Text(l),
                    // fgets gives NULL at the end of input and on an error
                    Ok(None) | Err(_) => Line::Eof,
                }
            }
            #[cfg(feature = "repl-line-editor")]
            Input::Editor(ed) => {
                // readline writes its prompt through stdout too: what that
                // holds goes out before it
                let _ = luna_core::stdio::flush_stdout();
                ed.read(&mut interp.vm, prompt)
            }
        }
    }

    /// `lua_saveline`: a build without readline keeps no history.
    #[cfg_attr(not(feature = "repl-line-editor"), allow(unused_variables))]
    pub(super) fn save(&mut self, line: &[u8]) {
        #[cfg(feature = "repl-line-editor")]
        if let Input::Editor(ed) = self
            && !line.is_empty()
        {
            ed.save(line);
        }
    }

    pub(super) fn finish(self) {
        match self {
            Input::Stdin => {}
            #[cfg(feature = "repl-line-editor")]
            Input::Editor(ed) => (*ed).finish(),
        }
    }
}
