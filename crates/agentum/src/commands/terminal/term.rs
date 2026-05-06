//! Wrapper around `vt100::Parser` for the live terminal pane.
//!
//! Tracks a scrollback offset on top of vt100's history so the
//! mouse-wheel / `Shift-PgUp` / `Shift-PgDn` flow can scroll through past
//! pane output without reaching for tmux's copy mode.

use vt100::Parser;

/// Lines kept in the vt100 history above the live screen. Larger than
/// most agents need so an autoscrolled `cargo build` log stays
/// reachable a few minutes later.
const SCROLLBACK: usize = 4096;

/// One scroll wheel "tick" should advance the view by this many lines.
/// Three matches Alacritty's default `mouse.scroll_lines` and lines up
/// with most users' muscle memory from kitty / iTerm.
pub const WHEEL_LINES_PER_TICK: usize = 3;

pub struct TerminalPane {
    parser: Parser,
    rows: u16,
    cols: u16,
    /// How many lines back from the live screen we're currently viewing.
    /// 0 = follow live output (default). Bumped by `scroll_up`, capped
    /// at the parser's known scrollback depth in `apply_scrollback`.
    scrollback_offset: usize,
}

impl TerminalPane {
    pub fn new() -> Self {
        let rows = 24;
        let cols = 80;
        Self {
            parser: Parser::new(rows, cols, SCROLLBACK),
            rows,
            cols,
            scrollback_offset: 0,
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.parser.set_size(rows, cols);
        self.rows = rows;
        self.cols = cols;
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn reset(&mut self) {
        self.parser = Parser::new(self.rows, self.cols, SCROLLBACK);
        self.scrollback_offset = 0;
    }

    /// True when the user has scrolled away from the live tail. The UI
    /// uses this to render an "↑ scrollback (n)" badge so it's obvious
    /// why fresh output isn't appearing.
    pub fn is_scrolled_back(&self) -> bool {
        self.scrollback_offset > 0
    }

    pub fn scrollback_offset(&self) -> usize {
        self.scrollback_offset
    }

    /// Move the view `n` lines up (toward older history). Clamped to
    /// vt100's actual scrollback depth so we never request a position
    /// past the oldest retained line.
    pub fn scroll_up(&mut self, n: usize) {
        // vt100 0.15 doesn't expose the precise scrollback depth, so we
        // use the configured `SCROLLBACK` as a cap. Set-and-clamp via
        // `set_scrollback` is the cheapest correct move: vt100 handles
        // the upper bound internally and silently clamps us.
        self.scrollback_offset = self.scrollback_offset.saturating_add(n).min(SCROLLBACK);
        self.apply_scrollback();
    }

    /// Move the view `n` lines down (toward live output). Saturating —
    /// hitting 0 means "at the live tail". Returns true if the view
    /// actually moved.
    pub fn scroll_down(&mut self, n: usize) -> bool {
        let prev = self.scrollback_offset;
        self.scrollback_offset = self.scrollback_offset.saturating_sub(n);
        if self.scrollback_offset != prev {
            self.apply_scrollback();
            true
        } else {
            false
        }
    }

    /// Snap back to following live output — used by the keystroke
    /// forwarder so any user input cancels scrollback (matches
    /// Alacritty / kitty behaviour).
    pub fn scroll_to_bottom(&mut self) {
        if self.scrollback_offset != 0 {
            self.scrollback_offset = 0;
            self.apply_scrollback();
        }
    }

    fn apply_scrollback(&mut self) {
        self.parser.set_scrollback(self.scrollback_offset);
    }
}
