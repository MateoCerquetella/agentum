//! Secrets at rest: "enc:" + Fernet token, the same format and key file
//! (.app_key) Odysseus used, so imported passwords and tokens just work.
use std::sync::OnceLock;

static KEY: OnceLock<fernet::Fernet> = OnceLock::new();

pub fn init(data_dir: &str) -> anyhow::Result<()> {
    let path = std::env::var("CERQ_APP_KEY").unwrap_or_else(|_| format!("{data_dir}/.app_key"));
    let key = match std::fs::read_to_string(&path) {
        Ok(k) => k.trim().to_string(),
        Err(_) => {
            let k = fernet::Fernet::generate_key();
            std::fs::write(&path, &k)?;
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?; }
            k
        }
    };
    let f = fernet::Fernet::new(&key).ok_or_else(|| anyhow::anyhow!("bad key in {path}"))?;
    let _ = KEY.set(f);
    Ok(())
}

/// Decrypt an "enc:" value; anything else is returned as is (legacy plaintext).
pub fn decrypt(v: &str) -> String {
    match v.strip_prefix("enc:") {
        Some(tok) => KEY.get().and_then(|k| k.decrypt(tok).ok()).and_then(|b| String::from_utf8(b).ok()).unwrap_or_default(),
        None => v.to_string(),
    }
}
pub fn encrypt(v: &str) -> String {
    if v.is_empty() { return String::new(); }
    format!("enc:{}", KEY.get().expect("secrets::init").encrypt(v.as_bytes()))
}
