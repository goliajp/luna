//! What a Windows pseudo console (ConPTY) shows, rebuilt from the bytes it
//! sends. ConPTY does not pass a program's output through: it renders its
//! own screen buffer and sends the VT sequences that paint it, choosing
//! among equivalent ones as it likes (a space may come as a space or as a
//! cursor move over a blank cell, a line may be repainted). Comparing the
//! bytes, or the bytes with the sequences taken out, depends on that
//! choice; comparing the screen they paint does not.
//!
//! The model keeps a viewport of the console's size, the rows scrolled off
//! its top, and the cursor. It follows the sequences that move the cursor
//! or change text, ignores those that only change attributes, modes or the
//! window title, and panics on any other, so that a new kind of output
//! fails the test plainly instead of rendering wrong.

/// The console's screen.
pub(crate) struct Screen {
    width: usize,
    height: usize,
    /// rows that scrolled off the top, oldest first
    scrolled: Vec<Vec<char>>,
    rows: Vec<Vec<char>>,
    row: usize,
    col: usize,
    saved: (usize, usize),
}

const BLANK: char = ' ';

impl Screen {
    pub(crate) fn new(width: usize, height: usize) -> Screen {
        Screen {
            width,
            height,
            scrolled: Vec::new(),
            rows: vec![Vec::new(); height],
            row: 0,
            col: 0,
            saved: (0, 0),
        }
    }

    /// The screen after `bytes` (UTF-8) were sent to a new console.
    pub(crate) fn render(bytes: &[u8], width: usize, height: usize) -> Screen {
        let mut s = Screen::new(width, height);
        s.feed(&String::from_utf8_lossy(bytes));
        s
    }

    fn put(&mut self, c: char) {
        if self.col >= self.width {
            self.col = 0;
            self.line_feed();
        }
        let line = &mut self.rows[self.row];
        if line.len() <= self.col {
            line.resize(self.col + 1, BLANK);
        }
        line[self.col] = c;
        self.col += 1;
    }

    fn line_feed(&mut self) {
        if self.row + 1 < self.height {
            self.row += 1;
        } else {
            let top = self.rows.remove(0);
            self.scrolled.push(top);
            self.rows.push(Vec::new());
        }
    }

    /// Blank the cells `from..to` of row `r`.
    fn erase(&mut self, r: usize, from: usize, to: usize) {
        let line = &mut self.rows[r];
        let to = to.min(line.len());
        for c in line.iter_mut().take(to).skip(from) {
            *c = BLANK;
        }
    }

    fn feed(&mut self, s: &str) {
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => self.col = 0,
                '\n' => self.line_feed(),
                '\x08' => self.col = self.col.saturating_sub(1),
                '\x07' => {}
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut fin = None;
                        for c in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&c) {
                                fin = Some(c);
                                break;
                            }
                            params.push(c);
                        }
                        let fin = fin.expect("a CSI sequence ends in a final byte");
                        self.csi(&params, fin);
                    }
                    Some(']') => {
                        // an operating system command (the window title):
                        // up to BEL or ST
                        while let Some(c) = chars.next() {
                            if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                                break;
                            }
                        }
                    }
                    Some('7') => self.saved = (self.row, self.col),
                    Some('8') => (self.row, self.col) = self.saved,
                    Some('M') => self.row = self.row.saturating_sub(1),
                    other => panic!("an escape sequence the screen does not know: ESC {other:?}"),
                },
                c if c < ' ' => panic!("a control character the screen does not know: {c:?}"),
                c => self.put(c),
            }
        }
    }

    fn csi(&mut self, params: &str, fin: char) {
        // `?` private modes and `>` / ` ` forms change no text
        let private = params.starts_with(['?', '>', '=', '<']) || params.ends_with(' ');
        let nums: Vec<usize> = params
            .trim_start_matches(['?', '>', '=', '<'])
            .split(';')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let n = |i: usize, default: usize| match nums.get(i) {
            Some(&0) | None => default,
            Some(&v) => v,
        };
        match fin {
            // attributes, modes, reports, cursor shape, window operations
            'm' | 'h' | 'l' | 'n' | 'c' | 't' | 'q' => {}
            _ if private => {
                panic!("a private CSI sequence the screen does not know: {params}{fin}")
            }
            'A' => self.row = self.row.saturating_sub(n(0, 1)),
            'B' => self.row = (self.row + n(0, 1)).min(self.height - 1),
            'C' => self.col = (self.col + n(0, 1)).min(self.width),
            'D' => self.col = self.col.saturating_sub(n(0, 1)),
            'E' => (self.row, self.col) = ((self.row + n(0, 1)).min(self.height - 1), 0),
            'F' => (self.row, self.col) = (self.row.saturating_sub(n(0, 1)), 0),
            'G' | '`' => self.col = (n(0, 1) - 1).min(self.width),
            'd' => self.row = (n(0, 1) - 1).min(self.height - 1),
            'H' | 'f' => {
                self.row = (n(0, 1) - 1).min(self.height - 1);
                self.col = (n(1, 1) - 1).min(self.width);
            }
            'K' => match nums.first().copied().unwrap_or(0) {
                0 => self.erase(self.row, self.col, usize::MAX),
                1 => self.erase(self.row, 0, self.col + 1),
                _ => self.erase(self.row, 0, usize::MAX),
            },
            'J' => match nums.first().copied().unwrap_or(0) {
                0 => {
                    self.erase(self.row, self.col, usize::MAX);
                    for r in self.row + 1..self.height {
                        self.rows[r].clear();
                    }
                }
                1 => {
                    for r in 0..self.row {
                        self.rows[r].clear();
                    }
                    self.erase(self.row, 0, self.col + 1);
                }
                3 => self.scrolled.clear(),
                _ => self.rows.iter_mut().for_each(Vec::clear),
            },
            'X' => self.erase(self.row, self.col, self.col + n(0, 1)),
            'P' => {
                let line = &mut self.rows[self.row];
                if self.col < line.len() {
                    let end = (self.col + n(0, 1)).min(line.len());
                    line.drain(self.col..end);
                }
            }
            '@' => {
                let line = &mut self.rows[self.row];
                if self.col < line.len() {
                    for _ in 0..n(0, 1) {
                        line.insert(self.col, BLANK);
                    }
                    line.truncate(self.width);
                }
            }
            'L' => {
                for _ in 0..n(0, 1) {
                    self.rows.insert(self.row, Vec::new());
                    self.rows.pop();
                }
            }
            'M' => {
                for _ in 0..n(0, 1) {
                    self.rows.remove(self.row);
                    self.rows.push(Vec::new());
                }
            }
            'S' => {
                for _ in 0..n(0, 1) {
                    let top = self.rows.remove(0);
                    self.scrolled.push(top);
                    self.rows.push(Vec::new());
                }
            }
            'T' => {
                for _ in 0..n(0, 1) {
                    self.rows.pop();
                    self.rows.insert(0, Vec::new());
                }
            }
            _ => panic!("a CSI sequence the screen does not know: {params}{fin}"),
        }
    }

    /// The text: every row that scrolled off and every row of the viewport
    /// down to the last one with text or the cursor, each without its
    /// trailing blanks except, on the cursor's row, up to the cursor;
    /// rows end in `\r\n`, the last one in nothing.
    pub(crate) fn text(&self) -> String {
        let last = self
            .rows
            .iter()
            .rposition(|r| r.iter().any(|&c| c != BLANK))
            .map_or(self.row, |r| r.max(self.row));
        let mut lines: Vec<String> = self.scrolled.iter().map(|r| trimmed(r, 0)).collect();
        for (i, r) in self.rows.iter().enumerate().take(last + 1) {
            let keep = if i == self.row { self.col } else { 0 };
            lines.push(trimmed(r, keep));
        }
        lines.join("\r\n")
    }
}

/// `row` without trailing blanks, but at least `keep` cells long.
fn trimmed(row: &[char], keep: usize) -> String {
    let end = row
        .iter()
        .rposition(|&c| c != BLANK)
        .map_or(0, |i| i + 1)
        .max(keep);
    let mut s: String = row.iter().take(end).collect();
    while s.chars().count() < end {
        s.push(BLANK);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::Screen;

    fn text(bytes: &str) -> String {
        Screen::render(bytes.as_bytes(), 20, 5).text()
    }

    #[test]
    fn plain_text_and_line_ends() {
        assert_eq!(text("ab\r\ncd"), "ab\r\ncd");
        assert_eq!(text("ab\r\n"), "ab\r\n");
        assert_eq!(text("ab\r\n\r\n"), "ab\r\n\r\n");
    }

    #[test]
    fn a_blank_reached_by_a_cursor_move_is_a_space() {
        // what ConPTY sent for the prompt on the run that failed
        assert_eq!(text(">\x1b[1C"), "> ");
        assert_eq!(text(">\x1b[1Cx"), "> x");
        assert_eq!(text("> "), "> ");
    }

    /// The start of what ConPTY sent on the run where the REPL's prompt
    /// came as `>` and a cursor move (develop CI run 37613146612).
    #[test]
    fn what_conpty_sent_for_a_prompt() {
        let raw = "\x1b[?9001h\x1b[?1004h\x1b[?25l\x1b[2J\x1b[m\x1b[H\
                   luna 4.0.2 (Lua 5.2)\r\n>\x1b[1C\x1b]0;D:\\a\\luna.exe\x07\x1b[?25h";
        assert_eq!(
            Screen::render(raw.as_bytes(), 120, 30).text(),
            "luna 4.0.2 (Lua 5.2)\r\n> "
        );
    }

    #[test]
    fn cursor_positions_and_erases() {
        assert_eq!(text("abc\x1b[1;2Hx"), "axc");
        assert_eq!(text("abcdef\x1b[3G\x1b[K"), "ab");
        assert_eq!(text("abcdef\x1b[3G\x1b[1K"), "   def");
        assert_eq!(text("abcdef\x1b[2K"), "      ");
        assert_eq!(text("abcdef\x1b[2D\x1b[2X"), "abcd");
        assert_eq!(text("abcdef\x1b[2D\x1b[2Xg"), "abcdg");
        assert_eq!(text("ab\r\ncd\x1b[H\x1b[J"), "");
        assert_eq!(text("ab\r\ncd\x1b[2J\x1b[Hz"), "z");
        assert_eq!(text("abcdef\x1b[3G\x1b[2P"), "abef");
    }

    #[test]
    fn attributes_modes_and_titles_change_nothing() {
        assert_eq!(
            text("\x1b[?25l\x1b[2m> \x1b[0mprint\x1b]0;title\x07(1)\x1b[?25h\x1b]0;t\x1b\\"),
            "> print(1)"
        );
    }

    #[test]
    fn rows_past_the_bottom_scroll() {
        assert_eq!(
            text("1\r\n2\r\n3\r\n4\r\n5\r\n6"),
            "1\r\n2\r\n3\r\n4\r\n5\r\n6"
        );
        // a position after scrolling is in the viewport
        assert_eq!(
            text("1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[1;1Hx"),
            "1\r\nx\r\n3\r\n4\r\n5\r\n6"
        );
    }

    #[test]
    fn a_long_line_wraps() {
        assert_eq!(
            text(&"a".repeat(25)),
            format!("{}\r\n{}", "a".repeat(20), "a".repeat(5))
        );
    }

    #[test]
    #[should_panic(expected = "does not know")]
    fn an_unknown_sequence_fails() {
        text("a\x1b[5Zb");
    }
}
