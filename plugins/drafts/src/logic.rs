//! A draft kept on the hub, and which copy wins.
use agentum_proto::Ms;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Draft {
    pub text: String,
    /// When it was typed (on the device that typed it).
    pub at: Ms,
}

/// On opening a composer: the hub's draft replaces the local text when it is newer and different.
pub fn take_hub(hub: Option<&Draft>, local: &str, local_at: Ms) -> Option<String> {
    let h = hub?;
    (h.at > local_at && h.text != local).then(|| h.text.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_hub_draft_wins() {
        let d = Draft { text: "from the laptop".into(), at: 200 };
        assert_eq!(take_hub(Some(&d), "", 0).as_deref(), Some("from the laptop"));
        assert_eq!(take_hub(Some(&d), "typed here later", 300), None);
        assert_eq!(take_hub(Some(&d), "from the laptop", 100), None);
        assert_eq!(take_hub(None, "x", 0), None);
    }
}
