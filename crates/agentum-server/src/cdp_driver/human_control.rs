//! Co-browse control arbitration (F12): tracks whether a human has grabbed the
//! wheel (manual co-browsing) so the agent yields control.
use super::*;

// --- co-browse control arbitration (F12) -------------------------------------

/// How long a human keeps the wheel after their last screencast input. Agent input
/// ops (click/fill) yield during this window so the two don't fight the same page.
const HUMAN_CONTROL_TTL: Duration = Duration::from_secs(5);

fn human_control_until() -> &'static Mutex<Option<std::time::Instant>> {
    static S: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Record human input (from the screencast pane): the human holds the wheel for
/// [`HUMAN_CONTROL_TTL`]. Called by the screencast route on real human actions.
pub fn note_human_input() {
    *human_control_until()
        .lock()
        .expect("control state poisoned") = Some(std::time::Instant::now() + HUMAN_CONTROL_TTL);
}

/// Whether a human currently holds the wheel (recent pane input, not expired).
pub fn human_has_control() -> bool {
    match *human_control_until()
        .lock()
        .expect("control state poisoned")
    {
        Some(until) => std::time::Instant::now() < until,
        None => false,
    }
}

/// The response when the agent tries to drive while the human holds the wheel.
pub(crate) fn human_has_control_response() -> Value {
    json!({ "ok": false, "error": "human_has_control" })
}

#[cfg(test)]
pub(crate) fn clear_human_control() {
    *human_control_until()
        .lock()
        .expect("control state poisoned") = None;
}

#[cfg(test)]
mod tests {
    use super::*;

        #[test]
        fn human_control_lock_grabs_and_releases() {
            clear_human_control();
            assert!(!human_has_control(), "no control by default");
            note_human_input();
            assert!(human_has_control(), "human holds the wheel after input");
            clear_human_control();
            assert!(!human_has_control(), "released after clear");
        }

        #[test]
        fn human_has_control_response_shape() {
            let v = human_has_control_response();
            assert_eq!(v["ok"], false);
            assert_eq!(v["error"], "human_has_control");
        }
}
