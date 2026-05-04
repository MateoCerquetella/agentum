//! Ensure `web/build/` exists at compile time so `rust-embed` always finds
//! something. Real bundle is produced by `pnpm --dir web build`; the stub
//! we drop here is a placeholder that prints a hint if the user forgets.

use std::path::Path;

fn main() {
    let dir = Path::new("../../web/build");
    let index = dir.join("index.html");

    if !index.exists() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("warning: could not create {}: {e}", dir.display());
            return;
        }
        let stub = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>agentum</title></head>
<body style="font-family:system-ui;padding:2rem;color:#888;background:#0a0a0c">
  <h1 style="color:#ff8a4c">agentum</h1>
  <p>Frontend bundle is missing. Run:</p>
  <pre style="background:#111;padding:1rem;border-radius:6px">pnpm --dir web install &amp;&amp; pnpm --dir web build</pre>
  <p>Then rebuild the server.</p>
</body>
</html>
"#;
        if let Err(e) = std::fs::write(&index, stub) {
            eprintln!("warning: could not write {}: {e}", index.display());
        }
    }

    println!("cargo:rerun-if-changed=../../web/build");
}
