use serde_json::Value;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub fn agent_trust_mark_trusted() {
    // Workspace trust is enforced by the agent CLIs themselves; marking it from here
    // isn't ported. Accept and no-op.
}

#[tauri::command]
pub fn developer_permissions_get_status() -> Vec<Value> {
    // OS developer-permission probing (accessibility, screen recording, …) isn't
    // ported; report none so the UI shows an empty list rather than erroring.
    Vec::new()
}

#[tauri::command]
pub async fn developer_permissions_open_settings(id: String) -> Result<(), String> {
    let _ = id; // The specific pane isn't routed yet; open the privacy/security root.
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        (
            "open",
            vec!["x-apple.systempreferences:com.apple.preference.security"],
        )
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", "ms-settings:privacy"])
    } else {
        ("xdg-open", vec!["gnome-control-center"])
    };
    tokio::process::Command::new(program)
        .args(&args)
        .status()
        .await
        .map_err(map_err)?;
    Ok(())
}

fn platform_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

// Computer Use OS grants (Accessibility + Screen Recording) are macOS-only and are
// probed against *this* process: TCC keys the grant to the running binary (the packaged
// agentum .app bundle), which is the same process that drives in-app computer-use.
#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::access::ScreenCaptureAccess;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        // Silent probe: reports whether this process holds the Accessibility grant.
        // Returns a Core Foundation `Boolean` (unsigned char), 0 = denied.
        fn AXIsProcessTrusted() -> std::os::raw::c_uchar;
        // Same probe, but `options` may carry kAXTrustedCheckOptionPrompt=true to
        // REGISTER this process in the Accessibility list and show the system prompt
        // when not yet trusted. This registration is the only way the app appears in
        // System Settings → Privacy → Accessibility — a plain deep-link never adds it.
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> std::os::raw::c_uchar;
        static kAXTrustedCheckOptionPrompt: CFStringRef;
    }

    pub fn accessibility_granted() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    // Register the app with TCC for Accessibility and surface the system prompt when
    // it isn't trusted yet. Safe to call when already granted (returns true, no prompt).
    pub fn request_accessibility() -> bool {
        unsafe {
            let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
            let options = CFDictionary::from_CFType_pairs(&[(
                key.as_CFType(),
                CFBoolean::true_value().as_CFType(),
            )]);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0
        }
    }

    pub fn screenshots_granted() -> bool {
        ScreenCaptureAccess::default().preflight()
    }

    // Fire the OS screen-recording prompt on first run; a no-op once granted.
    pub fn request_screenshots() {
        let _ = ScreenCaptureAccess::default().request();
    }

    pub fn status_str(granted: bool) -> &'static str {
        if granted {
            "granted"
        } else {
            "not-granted"
        }
    }
}

#[tauri::command]
pub fn computer_use_permissions_get_status() -> Value {
    #[cfg(target_os = "macos")]
    {
        serde_json::json!({
            "platform": platform_label(),
            "helperAppPath": null,
            "helperUnavailableReason": null,
            "permissions": [
                { "id": "accessibility", "status": macos::status_str(macos::accessibility_granted()) },
                { "id": "screenshots", "status": macos::status_str(macos::screenshots_granted()) }
            ]
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Other platforms have no equivalent TCC gate; report unsupported so the UI
        // hides the permission rows rather than nagging for a grant that can't exist.
        serde_json::json!({
            "platform": platform_label(),
            "helperAppPath": null,
            "helperUnavailableReason": null,
            "permissions": [
                { "id": "accessibility", "status": "unsupported" },
                { "id": "screenshots", "status": "unsupported" }
            ]
        })
    }
}

#[tauri::command]
pub async fn computer_use_permissions_open_setup(id: Option<String>) -> Result<Value, String> {
    let id = id.unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        // Poke the OS so the app registers with TCC and a first-run prompt appears,
        // then deep-link to the matching Privacy pane so the user lands on the right
        // toggle. The request_* calls are what make agentum show up in each list —
        // without them the pane stays empty no matter how often the user opens it.
        let anchor = if id == "screenshots" {
            macos::request_screenshots();
            "Privacy_ScreenCapture"
        } else {
            macos::request_accessibility();
            "Privacy_Accessibility"
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        tokio::process::Command::new("open")
            .arg(url)
            .status()
            .await
            .map_err(map_err)?;
        Ok(serde_json::json!({
            "platform": platform_label(),
            "helperAppPath": null,
            "permissionId": id,
            "openedSettings": true,
            "launchedHelper": true
        }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        Ok(serde_json::json!({
            "platform": platform_label(),
            "helperAppPath": null,
            "openedSettings": false,
            "launchedHelper": false
        }))
    }
}

#[tauri::command]
pub async fn computer_use_permissions_reset(app: tauri::AppHandle) -> Result<Value, String> {
    let bundle_id = app.config().identifier.clone();
    #[cfg(target_os = "macos")]
    {
        // Revoke this app's grants so the next probe reflects a clean slate.
        // Note: unbundled dev builds are TCC-keyed by binary path, so reset-by-bundle
        // can be a no-op there; it behaves correctly for the packaged .app.
        for service in ["Accessibility", "ScreenCapture"] {
            let _ = tokio::process::Command::new("tccutil")
                .args(["reset", service, &bundle_id])
                .status()
                .await;
        }
        Ok(serde_json::json!({
            "platform": platform_label(),
            "helperAppPath": null,
            "helperUnavailableReason": null,
            "permissions": [
                { "id": "accessibility", "status": macos::status_str(macos::accessibility_granted()) },
                { "id": "screenshots", "status": macos::status_str(macos::screenshots_granted()) }
            ],
            "bundleId": bundle_id
        }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(serde_json::json!({
            "platform": platform_label(),
            "helperAppPath": null,
            "helperUnavailableReason": null,
            "permissions": [
                { "id": "accessibility", "status": "unsupported" },
                { "id": "screenshots", "status": "unsupported" }
            ],
            "bundleId": bundle_id
        }))
    }
}

#[tauri::command]
pub fn developer_permissions_request(id: String) -> Value {
    // No OS permission probing; echo the id with an unsupported status.
    serde_json::json!({ "id": id, "status": "unsupported", "openedSystemSettings": false })
}
