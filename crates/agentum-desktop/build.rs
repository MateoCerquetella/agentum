fn main() {
    // The sherpa-rs/onnxruntime dylibs are built with `@rpath/...` install names
    // and copied next to our binary (target/<profile>/) by sherpa-rs-sys's build
    // script. Without an rpath the loader can't resolve them and the app crashes
    // on launch ("Library not loaded: @rpath/libonnxruntime…, no LC_RPATH's").
    //
    //   - `@loader_path`            → dev runs + dylibs sitting beside the binary
    //                                 (and Contents/MacOS in a bundled .app).
    //   - `@loader_path/../Frameworks` → the canonical .app location if the
    //                                 bundler stages them under Frameworks.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../Frameworks");
    }
    // Linux equivalent: search the binary's own directory for the .so files.
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }

    tauri_build::build()
}
