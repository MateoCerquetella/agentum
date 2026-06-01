fn main() {
    // Tauri codegen: parses tauri.conf.json, embeds the default window icon and
    // the (placeholder) frontendDist, and on Windows compiles the resource file.
    tauri_build::build();
}
