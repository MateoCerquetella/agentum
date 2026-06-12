//! macOS Accessibility (AX) implementation of the computer-use ops.
//!
//! Memory rule: Core Foundation "Copy"/"Create" functions return +1 references.
//! We wrap each returned ref via `wrap_under_create_rule` so it is released on
//! drop, and `wrap_under_get_rule` for borrowed refs. AX element refs we keep
//! only within a single call (walk + act together), so there is no cross-call
//! retention to manage.

use std::os::raw::c_void;

use core_foundation::array::CFArrayRef;
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{CGEvent, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use serde_json::{Value, json};

type AxUIElementRef = *const c_void;
type AxError = i32;
const AX_SUCCESS: AxError = 0;
/// Don't let a pathological tree (or a cycle the role guard misses) run away.
const MAX_ELEMENTS: usize = 2000;
const MAX_DEPTH: usize = 40;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AxUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AxUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AxError;
    fn AXUIElementPerformAction(element: AxUIElementRef, action: CFStringRef) -> AxError;
    fn AXUIElementSetAttributeValue(
        element: AxUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AxError;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
}

/// Copy a string-valued attribute, returning `None` when absent or non-string.
fn attr_string(element: AxUIElementRef, attr: &str) -> Option<String> {
    let key = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value)
    };
    if err != AX_SUCCESS || value.is_null() {
        return None;
    }
    unsafe {
        // Only interpret CFString values; release anything else and bail.
        if CFGetTypeID(value) == CFStringGetTypeID() {
            let s = CFString::wrap_under_create_rule(value as CFStringRef).to_string();
            Some(s)
        } else {
            CFRelease(value);
            None
        }
    }
}

/// Read children as a plain pointer vector with explicit retain: copy the
/// AXChildren array, CFRetain each element pointer (so it outlives the array),
/// then release the array. The caller owns and must release each returned ref.
fn children_via_indices(element: AxUIElementRef) -> Vec<AxUIElementRef> {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(arr: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: isize) -> *const c_void;
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    }
    let key = CFString::new("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value)
    };
    if err != AX_SUCCESS || value.is_null() {
        return Vec::new();
    }
    let arr = value as CFArrayRef;
    let mut out = Vec::new();
    unsafe {
        let n = CFArrayGetCount(arr);
        for i in 0..n {
            let item = CFArrayGetValueAtIndex(arr, i);
            if !item.is_null() {
                CFRetain(item as CFTypeRef);
                out.push(item as AxUIElementRef);
            }
        }
        CFRelease(value);
    }
    out
}

fn release(element: AxUIElementRef) {
    if !element.is_null() {
        unsafe { CFRelease(element as CFTypeRef) }
    }
}

/// One element's metadata in the flattened tree.
struct AxNode {
    role: String,
    title: String,
    value: String,
}

/// Walk an app's AX tree breadth-first, returning each element's ref + metadata
/// in a stable order. The caller must `release` every returned ref. The same
/// deterministic order backs both `get-app-state` (read) and `click`/`set-value`
/// (act by index) within a short window.
fn walk(pid: i32) -> Vec<(AxUIElementRef, AxNode)> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Vec::new();
    }
    let mut out: Vec<(AxUIElementRef, AxNode)> = Vec::new();
    // BFS queue of (element, depth). `app` itself isn't reported (index 0 is its
    // first child), but we keep it to seed the walk and release it at the end.
    let mut queue: std::collections::VecDeque<(AxUIElementRef, usize)> =
        std::collections::VecDeque::new();
    for child in children_via_indices(app) {
        queue.push_back((child, 1));
    }
    while let Some((el, depth)) = queue.pop_front() {
        if out.len() >= MAX_ELEMENTS {
            release(el);
            continue;
        }
        let node = AxNode {
            role: attr_string(el, "AXRole").unwrap_or_default(),
            title: attr_string(el, "AXTitle")
                .or_else(|| attr_string(el, "AXDescription"))
                .unwrap_or_default(),
            value: attr_string(el, "AXValue").unwrap_or_default(),
        };
        if depth < MAX_DEPTH {
            for child in children_via_indices(el) {
                queue.push_back((child, depth + 1));
            }
        }
        out.push((el, node));
    }
    release(app);
    out
}

fn release_all(elements: Vec<(AxUIElementRef, AxNode)>) {
    for (el, _) in elements {
        release(el);
    }
}

/// Resolve an app selector — `pid:<n>`, an exact-or-substring window owner name,
/// or a bundle id treated as a name fragment — to a pid.
fn resolve_pid(app: &str) -> Option<i32> {
    if let Some(rest) = app.strip_prefix("pid:") {
        return rest.trim().parse::<i32>().ok();
    }
    let want = app.to_lowercase();
    for (name, pid) in list_app_tuples() {
        let n = name.to_lowercase();
        if n == want || n.contains(&want) {
            return Some(pid);
        }
    }
    None
}

/// On-screen windows' owner (name, pid), de-duplicated by pid. Uses the public
/// window list (no AX grant needed for enumeration).
fn list_app_tuples() -> Vec<(String, i32)> {
    use core_graphics::window::{copy_window_info, kCGWindowListOptionOnScreenOnly};
    let mut seen = std::collections::BTreeMap::<i32, String>::new();
    if let Some(info) = copy_window_info(kCGWindowListOptionOnScreenOnly, 0) {
        // info is a CFArray of CFDictionary; read it via untyped CF calls to
        // avoid depending on a typed dictionary wrapper.
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFArrayGetCount(arr: CFArrayRef) -> isize;
            fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: isize) -> *const c_void;
            fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
            fn CFNumberGetValue(num: *const c_void, the_type: i32, value: *mut c_void) -> bool;
        }
        const KCF_NUMBER_INT: i32 = 9; // kCFNumberIntType
        let arr = info.as_concrete_TypeRef() as CFArrayRef;
        let name_key = CFString::new("kCGWindowOwnerName");
        let pid_key = CFString::new("kCGWindowOwnerPID");
        unsafe {
            let n = CFArrayGetCount(arr);
            for i in 0..n {
                let dict = CFArrayGetValueAtIndex(arr, i);
                if dict.is_null() {
                    continue;
                }
                let pid_ref =
                    CFDictionaryGetValue(dict, pid_key.as_concrete_TypeRef() as *const c_void);
                if pid_ref.is_null() {
                    continue;
                }
                let mut pid: i32 = 0;
                if !CFNumberGetValue(pid_ref, KCF_NUMBER_INT, &mut pid as *mut i32 as *mut c_void) {
                    continue;
                }
                let name_ref =
                    CFDictionaryGetValue(dict, name_key.as_concrete_TypeRef() as *const c_void);
                let name = if name_ref.is_null() {
                    String::new()
                } else {
                    CFString::wrap_under_get_rule(name_ref as CFStringRef).to_string()
                };
                if pid > 0 && !name.is_empty() {
                    seen.entry(pid).or_insert(name);
                }
            }
        }
    }
    seen.into_iter().map(|(pid, name)| (name, pid)).collect()
}

fn ax_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> std::os::raw::c_uchar;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Press the element at `index` in the app's flattened tree (`AXPress`).
fn perform_press(pid: i32, index: usize) -> anyhow::Result<()> {
    let els = walk(pid);
    let ok = if let Some((el, _)) = els.get(index) {
        let action = CFString::new("AXPress");
        unsafe { AXUIElementPerformAction(*el, action.as_concrete_TypeRef()) == AX_SUCCESS }
    } else {
        false
    };
    release_all(els);
    if ok {
        Ok(())
    } else {
        anyhow::bail!("could not press element {index}")
    }
}

fn set_value(pid: i32, index: usize, value: &str) -> anyhow::Result<()> {
    let els = walk(pid);
    let ok = if let Some((el, _)) = els.get(index) {
        let attr = CFString::new("AXValue");
        let v = CFString::new(value);
        unsafe {
            AXUIElementSetAttributeValue(
                *el,
                attr.as_concrete_TypeRef(),
                v.as_concrete_TypeRef() as CFTypeRef,
            ) == AX_SUCCESS
        }
    } else {
        false
    };
    release_all(els);
    if ok {
        Ok(())
    } else {
        anyhow::bail!("could not set value on element {index}")
    }
}

/// Type a unicode string into a specific process via a synthetic keyboard event.
fn type_text(pid: i32, text: &str) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("could not create event source"))?;
    let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
        .map_err(|_| anyhow::anyhow!("could not create key event"))?;
    down.set_string(text);
    down.post_to_pid(pid);
    Ok(())
}

/// Post a single key by virtual keycode to a process (down then up).
fn press_key(pid: i32, keycode: CGKeyCode) -> anyhow::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("could not create event source"))?;
    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|_| anyhow::anyhow!("key down"))?;
    down.post_to_pid(pid);
    let up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| anyhow::anyhow!("key up"))?;
    up.post_to_pid(pid);
    Ok(())
}

/// A few common key names → virtual keycodes. Extend as needed.
fn keycode_for(name: &str) -> Option<CGKeyCode> {
    Some(match name {
        "Return" | "Enter" => 36,
        "Tab" => 48,
        "Space" => 49,
        "Delete" | "Backspace" => 51,
        "Escape" | "Esc" => 53,
        "Left" => 123,
        "Right" => 124,
        "Down" => 125,
        "Up" => 126,
        _ => return None,
    })
}

pub fn handle(op: &str, args: &Value) -> anyhow::Result<Value> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let idx = || args.get("element-index").and_then(Value::as_u64).map(|v| v as usize);
    let resolve = |app: &str| {
        resolve_pid(app).ok_or_else(|| anyhow::anyhow!("no app matching `{app}` (try list-apps)"))
    };

    match op {
        "capabilities" => Ok(json!({
            "platform": "darwin",
            "accessibility": ax_trusted(),
            "ops": ["list-apps","get-app-state","click","set-value","type-text","press-key"],
        })),
        "permissions" => Ok(json!({
            "accessibility": if ax_trusted() { "granted" } else { "not-granted" },
        })),
        "list-apps" => {
            let apps: Vec<Value> = list_app_tuples()
                .into_iter()
                .map(|(name, pid)| json!({ "name": name, "pid": pid }))
                .collect();
            Ok(json!({ "apps": apps }))
        }
        "get-app-state" => {
            if !ax_trusted() {
                anyhow::bail!("Accessibility permission not granted for agentum");
            }
            let app = s("app").ok_or_else(|| anyhow::anyhow!("missing `app`"))?;
            let pid = resolve(&app)?;
            let els = walk(pid);
            let elements: Vec<Value> = els
                .iter()
                .enumerate()
                .map(|(i, (_, n))| {
                    json!({ "index": i, "role": n.role, "title": n.title, "value": n.value })
                })
                .collect();
            let count = elements.len();
            release_all(els);
            Ok(json!({ "app": app, "pid": pid, "count": count, "elements": elements }))
        }
        "click" => {
            let app = s("app").ok_or_else(|| anyhow::anyhow!("missing `app`"))?;
            let index = idx().ok_or_else(|| anyhow::anyhow!("missing `element-index`"))?;
            let pid = resolve(&app)?;
            perform_press(pid, index)?;
            Ok(json!({ "ok": true }))
        }
        "set-value" => {
            let app = s("app").ok_or_else(|| anyhow::anyhow!("missing `app`"))?;
            let index = idx().ok_or_else(|| anyhow::anyhow!("missing `element-index`"))?;
            let value = s("value").unwrap_or_default();
            let pid = resolve(&app)?;
            set_value(pid, index, &value)?;
            Ok(json!({ "ok": true }))
        }
        "type-text" => {
            let app = s("app").ok_or_else(|| anyhow::anyhow!("missing `app`"))?;
            let text = s("text").unwrap_or_default();
            let pid = resolve(&app)?;
            type_text(pid, &text)?;
            Ok(json!({ "ok": true }))
        }
        "press-key" => {
            let app = s("app").ok_or_else(|| anyhow::anyhow!("missing `app`"))?;
            let key = s("key").ok_or_else(|| anyhow::anyhow!("missing `key`"))?;
            let code = keycode_for(&key)
                .ok_or_else(|| anyhow::anyhow!("unknown key `{key}`"))?;
            let pid = resolve(&app)?;
            press_key(pid, code)?;
            Ok(json!({ "ok": true }))
        }
        other => Ok(json!({ "error": format!("unsupported computer op: {other}") })),
    }
}
