//! Continuation points for `read_file` after a provider truncated its output.
//!
//! Some providers (GigaChat on HTTP 413) cut a `read_file` result before the
//! model sees it. The provider records the first unseen line here, keyed by
//! the absolute path printed in the result footer; the next `read_file` of
//! that path without `start_line` resumes from it. Writes to the file and
//! context compaction invalidate the points. Process-wide, like the snippet
//! tracker in the tools crate.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn store() -> &'static Mutex<HashMap<String, usize>> {
    static STORE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record that the model has seen `path` up to `next_line - 1` (1-based).
pub fn set_next_line(path: &str, next_line: usize) {
    if let Ok(mut map) = store().lock() {
        map.insert(path.to_string(), next_line);
    }
}

/// Take (and remove) the continuation line for `path`, if any.
pub fn take_next_line(path: &str) -> Option<usize> {
    store().lock().ok()?.remove(path)
}

/// Drop the continuation point for `path` (the file changed or the model
/// navigated explicitly).
pub fn forget(path: &str) {
    if let Ok(mut map) = store().lock() {
        map.remove(path);
    }
}

/// Drop every continuation point (context compaction: the model no longer
/// holds the earlier parts it was continuing from).
pub fn clear() {
    if let Ok(mut map) = store().lock() {
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_take_forget_roundtrip() {
        let p = "/tmp/read-progress-roundtrip-test.md";
        set_next_line(p, 42);
        assert_eq!(take_next_line(p), Some(42));
        assert_eq!(take_next_line(p), None, "take consumes the point");
        set_next_line(p, 7);
        forget(p);
        assert_eq!(take_next_line(p), None);
    }
}
