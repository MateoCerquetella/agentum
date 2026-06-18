//! Keeps the `CString`s backing sherpa config pointers alive.
//!
//! The online recognizer config holds borrowed `*const c_char` pointers into our
//! strings; sherpa does not copy all of them, so they must outlive the
//! recognizer. `CString` owns a stable heap buffer, so pushing more strings (and
//! the holder's `Vec` reallocating) never moves an already-handed-out pointer.

use std::ffi::{c_char, CString};

#[derive(Default)]
pub struct CStringHolder {
    strings: Vec<CString>,
}

impl CStringHolder {
    /// Store a copy of `s` and return a pointer valid for the holder's lifetime.
    /// Interior NULs are stripped (paths/labels never legitimately contain them).
    pub fn push(&mut self, s: &str) -> *const c_char {
        let cleaned: String = s.chars().filter(|c| *c != '\0').collect();
        let c = CString::new(cleaned).unwrap_or_default();
        let ptr = c.as_ptr();
        self.strings.push(c);
        ptr
    }
}
