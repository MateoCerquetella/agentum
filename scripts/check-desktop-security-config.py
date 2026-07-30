#!/usr/bin/env python3
"""Fail closed when the desktop security boundary becomes broader."""

from __future__ import annotations

import json
import plistlib
import re
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
TAURI_CONFIG = ROOT / "crates" / "agentum-desktop" / "tauri.conf.json"
INFO_PLIST = ROOT / "crates" / "agentum-desktop" / "Info.plist"
ENTITLEMENTS_PLIST = ROOT / "crates" / "agentum-desktop" / "Entitlements.plist"
CAPABILITY = ROOT / "crates" / "agentum-desktop" / "capabilities" / "default.json"
BUILD_RS = ROOT / "crates" / "agentum-desktop" / "build.rs"
DESKTOP_LIB = ROOT / "crates" / "agentum-desktop" / "src" / "lib.rs"
APP_PERMISSION = (
    ROOT
    / "crates"
    / "agentum-desktop"
    / "permissions"
    / "main-webview-commands.toml"
)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(message)


def parse_csp(raw: str) -> dict[str, list[str]]:
    directives: dict[str, list[str]] = {}
    for item in raw.split(";"):
        words = item.strip().split()
        if not words:
            continue
        require(words[0] not in directives, f"duplicate CSP directive: {words[0]}")
        directives[words[0]] = words[1:]
    return directives


config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
require(config.get("productName") == "Agentum", "unexpected desktop product name")
require(config.get("identifier") == "dev.agentum.app", "unexpected desktop bundle identifier")

build = config.get("build", {})
require(build.get("frontendDist") == "ui/dist", "desktop frontend must remain bundled")
require(build.get("devUrl") == "http://127.0.0.1:1420", "desktop dev URL must remain loopback-only")

csp_raw = config.get("app", {}).get("security", {}).get("csp")
require(isinstance(csp_raw, str) and csp_raw.strip(), "desktop CSP must be explicit")
csp = parse_csp(csp_raw)

require(csp.get("default-src") == ["'self'", "customprotocol:", "asset:"], "unexpected default-src")
require(csp.get("base-uri") == ["'self'"], "base-uri must be self only")
require(csp.get("object-src") == ["'none'"], "object embedding must remain disabled")
require(csp.get("frame-ancestors") == ["'none'"], "renderer must not be framed")
require(csp.get("form-action") == ["'none'"], "renderer form submissions must remain disabled")
require(csp.get("script-src") == ["'self'"], "renderer scripts must be bundled")
require("'unsafe-eval'" not in csp.get("script-src", []), "unsafe eval is forbidden")

expected_connect = [
    "'self'",
    "ipc:",
    "http://ipc.localhost",
    "http://127.0.0.1:*",
    "ws://127.0.0.1:*",
]
require(csp.get("connect-src") == expected_connect, "connect-src must remain loopback/IPC only")

images = csp.get("img-src", [])
require("https:" in images, "HTTPS images are required for dynamic work-item avatars and Markdown")
require("http:" not in images and not any(value.startswith("http://") and value != "http://asset.localhost" for value in images), "remote plaintext images are forbidden")

updater = config.get("plugins", {}).get("updater", {})
require(
    updater.get("endpoints")
    == ["https://github.com/MateoCerquetella/agentum/releases/latest/download/latest.json"],
    "updater endpoint changed without security review",
)
require(bool(updater.get("pubkey")), "updater verification key is required")
require(
    updater.get("windows", {}).get("installMode") == "passive",
    "Windows updater install mode must remain passive",
)

bundle = config.get("bundle", {})
require(bundle.get("active") is True, "desktop bundling must remain enabled")
require(bundle.get("targets") == "all", "desktop bundle target policy changed")
require(bundle.get("createUpdaterArtifacts") is True, "signed updater artifacts are required")
expected_icons = [
    "icons/32x32.png",
    "icons/128x128.png",
    "icons/128x128@2x.png",
    "icons/icon.icns",
    "icons/icon.ico",
]
require(bundle.get("icon") == expected_icons, "desktop icon roster changed")
for icon in expected_icons:
    require((TAURI_CONFIG.parent / icon).is_file(), f"desktop icon is missing: {icon}")
macos = bundle.get("macOS", {})
require(macos.get("infoPlist") == "Info.plist", "macOS Info.plist override changed")
require(
    macos.get("entitlements") == "Entitlements.plist",
    "macOS hardened-runtime entitlements changed",
)
require(macos.get("hardenedRuntime") is True, "macOS hardened runtime is required")
require(macos.get("minimumSystemVersion") == "11.0", "unexpected macOS deployment target")
with ENTITLEMENTS_PLIST.open("rb") as handle:
    entitlements = plistlib.load(handle)
require(
    entitlements == {"com.apple.security.device.audio-input": True},
    "macOS entitlements must grant only microphone input",
)
linux = bundle.get("linux", {})
require(
    linux.get("deb", {}).get("depends") == ["bubblewrap"],
    "Debian package must require the SDD process sandbox",
)
require(
    linux.get("rpm", {}).get("depends") == ["bubblewrap"],
    "RPM package must require the SDD process sandbox",
)
windows = bundle.get("windows", {})
require(
    not {"certificateThumbprint", "digestAlgorithm", "timestampUrl"}.intersection(windows),
    "Windows Authenticode configuration must remain absent without a certificate",
)

with INFO_PLIST.open("rb") as handle:
    plist = plistlib.load(handle)
ats = plist.get("NSAppTransportSecurity", {})
require(ats.get("NSAllowsLocalNetworking") is True, "loopback ATS exception is required")
require(ats.get("NSAllowsArbitraryLoads") is not True, "arbitrary ATS loads are forbidden")
require(ats.get("NSAllowsArbitraryLoadsInWebContent") is not True, "arbitrary web ATS loads are forbidden")
require(bool(plist.get("NSMicrophoneUsageDescription")), "microphone usage text is required")

capability = json.loads(CAPABILITY.read_text(encoding="utf-8"))
require(
    capability.get("webviews") == ["main"] and "windows" not in capability,
    "desktop capability must target only the trusted main webview",
)
require(
    capability.get("permissions")
    == [
        "core:default",
        "core:window:allow-start-dragging",
        "main-webview-commands",
    ],
    "desktop capability permissions changed without review",
)

build_rs = BUILD_RS.read_text(encoding="utf-8")
require(
    "app_manifest(tauri_build::AppManifest::new())" in build_rs
    and ".commands(" not in build_rs
    and "tauri_build::try_build(attributes)" in build_rs,
    "Tauri build must load and enforce the complete application permission manifest",
)

desktop_lib = DESKTOP_LIB.read_text(encoding="utf-8")
handler_match = re.search(
    r"tauri::generate_handler!\[(?P<body>.*?)\]\)",
    desktop_lib,
    flags=re.DOTALL,
)
require(handler_match is not None, "desktop generate_handler command list is missing")
handler_commands = re.findall(
    r"^[ \t]*(?:[A-Za-z_][A-Za-z0-9_]*::)+([A-Za-z_][A-Za-z0-9_]*)[ \t]*,[ \t]*$",
    handler_match.group("body"),
    flags=re.MULTILINE,
)
require(handler_commands, "desktop generate_handler command list is empty")
require(
    len(handler_commands) == len(set(handler_commands)),
    "desktop generate_handler contains a duplicate command name",
)

with APP_PERMISSION.open("rb") as handle:
    permission_file = tomllib.load(handle)
permissions = permission_file.get("permission", [])
require(
    isinstance(permissions, list) and len(permissions) == 1,
    "desktop application permission must contain exactly one explicit permission",
)
permission = permissions[0]
require(
    permission.get("identifier") == "main-webview-commands",
    "unexpected desktop application permission identifier",
)
allowed_commands = permission.get("commands", {}).get("allow")
require(
    isinstance(allowed_commands, list) and all(isinstance(value, str) for value in allowed_commands),
    "desktop application permission command allowlist is malformed",
)
require(
    len(allowed_commands) == len(set(allowed_commands)),
    "desktop application permission contains duplicate commands",
)
require(
    set(allowed_commands) == set(handler_commands),
    "desktop application ACL must exactly match every generate_handler command",
)
require(
    allowed_commands == sorted(allowed_commands),
    "desktop application ACL must remain deterministically sorted",
)
require(
    "app_get_server_endpoint" in allowed_commands,
    "embedded API endpoint capability is missing from the desktop ACL",
)

print("desktop security configuration: ok")
