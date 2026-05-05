//! Wrapper around `vt100::Parser` for the live terminal pane.

use vt100::Parser;

const SCROLLBACK: usize = 4096;

pub struct TerminalPane {
    parser: Parser,
    rows: u16,
    cols: u16,
}

impl TerminalPane {
    pub fn new() -> Self {
        let rows = 24;
        let cols = 80;
        Self {
            parser: Parser::new(rows, cols, SCROLLBACK),
            rows,
            cols,
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
    }
}
