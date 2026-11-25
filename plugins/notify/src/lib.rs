//! Notifications: A push when a thread needs you or finishes.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "notify",
    name: "Notifications",
    description: "A push when a thread needs you or finishes.",
    icon: "bell",
    group: Group::Usage,
    default_on: true,
    settings: &[
        SettingDef { key: "on_ask", label: "When a thread asks", help: "A question or a permission prompt.", kind: SettingKind::Bool, default: "true" },
        SettingDef { key: "on_done", label: "When a turn ends", help: "Finished or failed (not when you stopped it).", kind: SettingKind::Bool, default: "true" },
        SettingDef { key: "quiet_from", label: "Quiet from", help: "HH:MM on the hub's clock; empty: never quiet.", kind: SettingKind::Text, default: "\"\"" },
        SettingDef { key: "quiet_to", label: "Quiet until", help: "HH:MM; quiet hours may cross midnight (22:00 to 07:00).", kind: SettingKind::Text, default: "\"\"" },
    ],
};

/// Are minutes-of-day `now` inside quiet hours `from`..`to` (which may wrap past midnight)?
pub fn quiet(now: u32, from: Option<u32>, to: Option<u32>) -> bool {
    match (from, to) {
        (Some(f), Some(t)) if f < t => (f..t).contains(&now),
        (Some(f), Some(t)) if f > t => now >= f || now < t,
        _ => false,
    }
}

/// "22:30" → minutes of the day.
pub fn hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

#[cfg(test)]
mod tests {
    #[test]
    fn quiet_hours() {
        use super::{hhmm, quiet};
        assert_eq!(hhmm("07:05"), Some(425));
        assert_eq!(hhmm("25:00"), None);
        assert_eq!(hhmm(""), None);
        let (f, t) = (hhmm("22:00"), hhmm("07:00"));
        assert!(quiet(23 * 60, f, t) && quiet(60, f, t) && !quiet(12 * 60, f, t) && !quiet(7 * 60, f, t));
        assert!(quiet(13 * 60, hhmm("12:00"), hhmm("14:00")) && !quiet(15 * 60, hhmm("12:00"), hhmm("14:00")));
        assert!(!quiet(0, None, t));
    }
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
