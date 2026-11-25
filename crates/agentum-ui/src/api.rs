//! JSON over HTTP to the hub. A 401 (outside /api/auth) signs the page out.
use gloo_net::http::{Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}

thread_local! {
    static ON_LOGOUT: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
}
/// What to do when the hub says we're signed out (the app shows the login screen).
pub fn on_logout(f: impl Fn() + 'static) {
    ON_LOGOUT.with(|c| *c.borrow_mut() = Some(Box::new(f)));
}

/// `api("/mail/list", "GET", None)`: the path is under `/api`.
pub async fn api<T: DeserializeOwned>(path: &str, method: &str, body: Option<&Value>) -> Result<T, ApiError> {
    let m = match method {
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "DELETE" => Method::DELETE,
        "PATCH" => Method::PATCH,
        _ => Method::GET,
    };
    let url = format!("/api{path}");
    let req = RequestBuilder::new(&url).method(m).credentials(web_sys::RequestCredentials::SameOrigin);
    let res = match body {
        Some(b) => req.header("Content-Type", "application/json").body(b.to_string()).map_err(err0)?.send().await,
        None => req.send().await,
    }
    .map_err(err0)?;
    let status = res.status();
    if status == 401 && !path.starts_with("/auth") {
        ON_LOGOUT.with(|c| {
            if let Some(f) = c.borrow().as_ref() {
                f()
            }
        });
    }
    let text = res.text().await.map_err(err0)?;
    let parsed: Value = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text.clone())) };
    if !(200..300).contains(&status) {
        let message = parsed.get("error").and_then(|e| e.as_str()).map(str::to_string).unwrap_or_else(|| res.status_text());
        return Err(ApiError { status, message });
    }
    serde_json::from_value(parsed).map_err(|e| ApiError { status, message: format!("unexpected answer from {path}: {e}") })
}

fn err0(e: impl fmt::Display) -> ApiError {
    ApiError { status: 0, message: e.to_string() }
}

pub async fn api_get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    api(path, "GET", None).await
}
pub async fn api_post<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, ApiError> {
    api(path, "POST", Some(&serde_json::to_value(body).unwrap_or(Value::Null))).await
}
pub async fn api_put<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, ApiError> {
    api(path, "PUT", Some(&serde_json::to_value(body).unwrap_or(Value::Null))).await
}
pub async fn api_delete<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    api(path, "DELETE", None).await
}

/// A plugin's hub part: `POST /api/p/<plugin>/<method>`.
pub async fn rpc<T: DeserializeOwned>(plugin: &str, method: &str, input: &impl Serialize) -> Result<T, ApiError> {
    api_post(&format!("/p/{plugin}/{method}"), input).await
}
