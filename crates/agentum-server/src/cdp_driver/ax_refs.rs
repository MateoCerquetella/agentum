//! Accessibility refs: snapshot the AX tree into opaque, generation-stamped
//! refs the agent acts on (click/type), resolving back to CDP backendNodeIds.
use super::*;

// --- accessibility refs (snapshot → opaque refs the agent acts on) -----------

/// Interactive AX roles surfaced as actionable refs.
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "checkbox",
    "radio",
    "switch",
    "slider",
    "spinbutton",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "option",
    "textarea",
];

/// Roles useful even without an accessible name (a bare input an agent types into).
const NAMELESS_OK_ROLES: &[&str] = &["textbox", "searchbox", "textarea", "combobox"];

/// Cap on refs returned by one snapshot, so a huge page can't blow up the response.
const MAX_REFS: usize = 250;

/// One interactive element from a snapshot. `ref_id` is opaque
/// (`e{generation}_{idx}`) and resolves to a `backendDOMNodeId` via [`ref_registry`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AxRef {
    ref_id: String,
    role: String,
    name: String,
    value: Option<String>,
    disabled: Option<bool>,
    checked: Option<String>,
    backend_node_id: i64,
}

/// Server-side ref→backendNodeId map for the latest snapshot. The CDP browser is a
/// per-machine singleton, so one global registry mirrors it. A new snapshot bumps
/// `generation` and replaces `map`; a ref carrying a stale generation isn't in the
/// current map, so it resolves to `None` → the action returns `stale_ref`.
#[derive(Default)]
struct RefRegistry {
    generation: u64,
    map: HashMap<String, i64>,
}

impl RefRegistry {
    /// Store the ref→backendNodeId map for `generation`, unless a newer snapshot
    /// has already superseded it (then its map wins and these refs read as stale).
    fn store(&mut self, generation: u64, refs: &[AxRef]) {
        if self.generation == generation {
            self.map = refs
                .iter()
                .map(|r| (r.ref_id.clone(), r.backend_node_id))
                .collect();
        }
    }

    /// Resolve a ref to its backendNodeId, or `None` when stale (wrong generation,
    /// or the page moved on so the ref isn't in the current map).
    fn resolve(&self, ref_id: &str) -> Option<i64> {
        self.map.get(ref_id).copied()
    }
}

fn ref_registry() -> &'static Mutex<RefRegistry> {
    static REG: OnceLock<Mutex<RefRegistry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(RefRegistry::default()))
}

/// Allocate the next snapshot generation.
pub(crate) fn next_generation() -> u64 {
    let mut reg = ref_registry().lock().expect("ref registry poisoned");
    reg.generation += 1;
    reg.generation
}

/// The current snapshot generation without bumping it — used to stamp console /
/// network entries so `get_console(since_generation)` can return "what happened
/// since my last snapshot".
pub(crate) fn current_generation() -> u64 {
    ref_registry()
        .lock()
        .expect("ref registry poisoned")
        .generation
}

pub(crate) fn store_refs(generation: u64, refs: &[AxRef]) {
    ref_registry()
        .lock()
        .expect("ref registry poisoned")
        .store(generation, refs);
}

pub(crate) fn resolve_ref(ref_id: &str) -> Option<i64> {
    ref_registry()
        .lock()
        .expect("ref registry poisoned")
        .resolve(ref_id)
}

/// Read an AX node `properties[].value.value` for `key`.
fn ax_property(node: &Value, key: &str) -> Option<Value> {
    node.get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(key))?
        .get("value")?
        .get("value")
        .cloned()
}

/// Parse a CDP `Accessibility.getFullAXTree` result into ref entries for
/// `generation`. `interactive_only` keeps only actionable roles (the default).
/// Capped at [`MAX_REFS`]; the returned `bool` is `true` when truncated.
pub(crate) fn parse_ax_refs(
    tree: &Value,
    generation: u64,
    interactive_only: bool,
) -> (Vec<AxRef>, bool) {
    let mut out = Vec::new();
    let Some(nodes) = tree.get("nodes").and_then(Value::as_array) else {
        return (out, false);
    };
    for node in nodes {
        if node.get("ignored").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(backend_node_id) = node.get("backendDOMNodeId").and_then(Value::as_i64) else {
            continue;
        };
        let role = node
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if interactive_only && !INTERACTIVE_ROLES.contains(&role.as_str()) {
            continue;
        }
        let name = node
            .get("name")
            .and_then(|n| n.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // A nameless element is rarely actionable — except bare inputs, which an
        // agent still needs to type into.
        if interactive_only && name.is_empty() && !NAMELESS_OK_ROLES.contains(&role.as_str()) {
            continue;
        }
        if out.len() >= MAX_REFS {
            return (out, true);
        }
        let value = node
            .get("value")
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let disabled = ax_property(node, "disabled").and_then(|v| v.as_bool());
        let checked = ax_property(node, "checked").map(|v| match v {
            Value::Bool(b) => b.to_string(),
            Value::String(s) => s,
            other => other.to_string(),
        });
        out.push(AxRef {
            ref_id: format!("e{generation}_{}", out.len() + 1),
            role,
            name,
            value,
            disabled,
            checked,
            backend_node_id,
        });
    }
    (out, false)
}

/// Public JSON for a ref (drops the internal backendNodeId).
pub(crate) fn ax_ref_public(r: &AxRef) -> Value {
    let mut o = json!({ "ref": r.ref_id, "role": r.role, "name": r.name });
    if let Some(v) = &r.value {
        o["value"] = json!(v);
    }
    if let Some(d) = r.disabled {
        o["disabled"] = json!(d);
    }
    if let Some(c) = &r.checked {
        o["checked"] = json!(c);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ax_tree() -> Value {
        json!({ "nodes": [
            { "ignored": false, "role": {"value":"button"}, "name": {"value":"Submit"},
              "backendDOMNodeId": 10, "properties": [{"name":"disabled","value":{"value":false}}] },
            { "ignored": false, "role": {"value":"textbox"}, "name": {"value":""},
              "value": {"value":"hi"}, "backendDOMNodeId": 11 },
            { "ignored": false, "role": {"value":"checkbox"}, "name": {"value":"Agree"},
              "backendDOMNodeId": 12, "properties": [{"name":"checked","value":{"value":"true"}}] },
            { "ignored": true, "role": {"value":"button"}, "name": {"value":"Hidden"},
              "backendDOMNodeId": 13 },
            { "ignored": false, "role": {"value":"StaticText"}, "name": {"value":"label"},
              "backendDOMNodeId": 14 },
            { "ignored": false, "role": {"value":"generic"}, "name": {"value":""},
              "backendDOMNodeId": 15 }
        ]})
    }

    #[test]
    fn parse_ax_refs_filters_to_interactive_and_extracts_fields() {
        let (refs, truncated) = parse_ax_refs(&sample_ax_tree(), 7, true);
        assert!(!truncated);
        // button + nameless textbox (kept) + checkbox; ignored/StaticText/generic dropped.
        assert_eq!(refs.len(), 3);

        assert_eq!(refs[0].ref_id, "e7_1");
        assert_eq!(refs[0].role, "button");
        assert_eq!(refs[0].name, "Submit");
        assert_eq!(refs[0].disabled, Some(false));
        assert_eq!(refs[0].backend_node_id, 10);

        assert_eq!(refs[1].ref_id, "e7_2");
        assert_eq!(refs[1].role, "textbox");
        assert_eq!(refs[1].value.as_deref(), Some("hi"));

        assert_eq!(refs[2].ref_id, "e7_3");
        assert_eq!(refs[2].role, "checkbox");
        assert_eq!(refs[2].checked.as_deref(), Some("true"));
    }

    #[test]
    fn parse_ax_refs_full_mode_includes_noninteractive() {
        let (interactive, _) = parse_ax_refs(&sample_ax_tree(), 1, true);
        let (full, _) = parse_ax_refs(&sample_ax_tree(), 1, false);
        assert!(
            full.len() > interactive.len(),
            "full mode surfaces more nodes"
        );
    }

    #[test]
    fn ax_ref_public_omits_backend_id_and_absent_optionals() {
        let r = AxRef {
            ref_id: "e1_1".into(),
            role: "link".into(),
            name: "Home".into(),
            value: None,
            disabled: None,
            checked: None,
            backend_node_id: 99,
        };
        let v = ax_ref_public(&r);
        assert_eq!(v["ref"], "e1_1");
        assert_eq!(v["role"], "link");
        assert_eq!(v["name"], "Home");
        assert!(v.get("value").is_none());
        assert!(v.get("disabled").is_none());
        // The internal backendNodeId is never exposed to the agent.
        assert!(v.get("backend_node_id").is_none());
        assert!(v.get("backendNodeId").is_none());
    }

    #[test]
    fn ref_registry_resolves_current_generation_and_rejects_stale() {
        let refs = vec![AxRef {
            ref_id: "e5_1".into(),
            role: "button".into(),
            name: "Go".into(),
            value: None,
            disabled: None,
            checked: None,
            backend_node_id: 42,
        }];
        let mut reg = RefRegistry {
            generation: 5,
            map: HashMap::new(),
        };
        reg.store(5, &refs);
        assert_eq!(reg.resolve("e5_1"), Some(42));
        // A ref from another generation isn't in the map → stale.
        assert_eq!(reg.resolve("e4_1"), None);

        // A store for a superseded generation is ignored (newer snapshot wins).
        reg.generation = 6;
        let other = vec![AxRef {
            ref_id: "e5_1".into(),
            role: "button".into(),
            name: "Go".into(),
            value: None,
            disabled: None,
            checked: None,
            backend_node_id: 999,
        }];
        reg.store(5, &other);
        assert_eq!(
            reg.resolve("e5_1"),
            Some(42),
            "a stale-generation store must not overwrite the current map"
        );
    }
}
