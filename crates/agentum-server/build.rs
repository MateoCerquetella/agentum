//! Ensure `dashboard/build/` exists at compile time so `rust-embed` always
//! finds something. Real bundle is produced by `pnpm --dir dashboard build`;
//! the stub we drop here is a placeholder that prints a hint if the user
//! forgets.

use std::path::Path;

fn main() {
    let dir = Path::new("../../dashboard/build");
    let index = dir.join("index.html");

    if !index.exists() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("warning: could not create {}: {e}", dir.display());
            return;
        }
        let stub = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>agentum</title></head>
<body style="font-family:system-ui;padding:2rem;color:#b9b9b9;background:#0b0b0b">
  <h1 style="color:#f36458">agentum</h1>
  <p>Dashboard bundle is missing. Run:</p>
  <pre style="background:#212121;padding:1rem;border-radius:6px;color:#b9b9b9">pnpm --dir dashboard install &amp;&amp; pnpm --dir dashboard build</pre>
  <p>Then rebuild the server.</p>
</body>
</html>
"#;
        if let Err(e) = std::fs::write(&index, stub) {
            eprintln!("warning: could not write {}: {e}", index.display());
        }
    }

    println!("cargo:rerun-if-changed=../../dashboard/build");
}
