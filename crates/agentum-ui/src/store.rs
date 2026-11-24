//! localStorage (this device only).

fn ls() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}
pub fn get(key: &str) -> Option<String> {
    ls()?.get_item(key).ok()?
}
pub fn set(key: &str, value: &str) {
    if let Some(s) = ls() {
        let _ = s.set_item(key, value);
    }
}
pub fn remove(key: &str) {
    if let Some(s) = ls() {
        let _ = s.remove_item(key);
    }
}
pub fn flag(key: &str) -> bool {
    get(key).is_some_and(|v| !v.is_empty() && v != "0")
}
