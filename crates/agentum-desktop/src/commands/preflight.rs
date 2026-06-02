use serde_json::{json, Value};

// Preflight tool detection. The renderer reads `status.git.installed`,
// `status.glab.installed`, etc., so the result MUST carry the full PreflightStatus
// shape (mirrors web-preload-api `fallbackStatus`) — returning a partial object makes
// the Landing page throw `undefined is not an object (evaluating 's.git.installed')`.
// `installed` is detected on PATH; provider auth/account checks aren't ported yet, so
// they report unauthenticated/unconfigured.
#[tauri::command]
pub fn preflight_check() -> Value {
    let installed = |bin: &str| which::which(bin).is_ok();
    json!({
        "git": { "installed": installed("git") },
        "gh": { "installed": installed("gh"), "authenticated": false },
        "glab": { "installed": installed("glab"), "authenticated": false },
        "bitbucket": { "configured": false, "authenticated": false, "account": null },
        "azureDevOps": {
            "configured": false,
            "authenticated": false,
            "account": null,
            "baseUrl": null,
            "tokenConfigured": false
        },
        "gitea": {
            "configured": false,
            "authenticated": false,
            "account": null,
            "baseUrl": null,
            "tokenConfigured": false
        }
    })
}
