use std::io::{self, Write};

use anyhow::{Result, anyhow};

use crate::cli::AuthCmd;

pub async fn run(action: AuthCmd) -> Result<()> {
    let (store, _) = super::open_store().await?;

    match action {
        AuthCmd::List => {
            let users = store.list_users().await?;
            if users.is_empty() {
                println!("(no users — run `agentum auth setup` to create the first admin)");
                return Ok(());
            }
            for u in users {
                println!("{:>4}  {:<24}  {}", u.id, u.username, u.created_at);
            }
            Ok(())
        }
        AuthCmd::Add { username, password } => {
            let username = username.trim().to_lowercase();
            agentum_core::validate_username(&username)
                .map_err(|e| anyhow!("invalid username: {e}"))?;
            let pw = match password {
                Some(p) => p,
                None => prompt_password("password")?,
            };
            if pw.len() < 8 {
                return Err(anyhow!("password must be at least 8 characters"));
            }
            let hash = agentum_server::auth::hash_password(pw)
                .await
                .map_err(|e| anyhow!("hash failed: {e}"))?;
            let user = store.create_user(&username, &hash).await?;
            println!("created user {} (id={})", user.username, user.id);
            Ok(())
        }
        AuthCmd::Rm { username } => {
            let username = username.trim().to_lowercase();
            if !store.delete_user_by_username(&username).await? {
                return Err(anyhow!("no user named {username}"));
            }
            println!("removed user {username}");
            Ok(())
        }
        AuthCmd::Reset => {
            store.wipe_users().await?;
            println!("auth reset — visit the dashboard to register again");
            Ok(())
        }
        AuthCmd::Setup { username, password } => run_setup_wizard(&store, username, password).await,
    }
}

/// Interactive (or non-interactive) first-time setup wizard.
///
/// Called from `agentum auth setup` and from `agentum serve` on first boot
/// when zero users exist. Writes the admin account directly to the store.
pub(crate) async fn run_setup_wizard(
    store: &agentum_store::Store,
    username: Option<String>,
    password: Option<String>,
) -> Result<()> {
    let count = store.count_users().await?;
    let non_interactive = username.is_some() && password.is_some();

    if count > 0 && !non_interactive {
        eprintln!();
        eprintln!("  ⚠  {} user(s) already exist.", count);
        eprintln!("     This will add another admin account.");
        eprintln!("     Use `agentum auth reset` first to start fresh.");
        eprintln!();
        eprint!("  Continue? [y/N]: ");
        io::stderr().flush().ok();
        let mut ans = String::new();
        io::stdin().read_line(&mut ans)?;
        if !ans.trim().eq_ignore_ascii_case("y") {
            eprintln!("  Cancelled.");
            return Ok(());
        }
    }

    if non_interactive {
        let u = username.unwrap().trim().to_lowercase();
        let p = password.unwrap();
        agentum_core::validate_username(&u).map_err(|e| anyhow!("invalid username: {e}"))?;
        if p.len() < 8 {
            return Err(anyhow!("password must be at least 8 characters"));
        }
        let hash = agentum_server::auth::hash_password(p)
            .await
            .map_err(|e| anyhow!("hash failed: {e}"))?;
        let user = store.create_user(&u, &hash).await?;
        println!("created admin user {} (id={})", user.username, user.id);
        return Ok(());
    }

    // Interactive wizard.
    //
    // All output goes to stdout (not stderr) so prompts and the
    // surrounding boxes share a single, line-buffered stream — the
    // prior split caused prompts to collide on one line under some
    // terminal modes (`curl | sh` + `</dev/tty`), which made the
    // password step look like it had been skipped.
    println!();
    println!("  ┌─────────────────────────────────────────┐");
    println!("  │  Create your admin account              │");
    println!("  │  (used for the dashboard and TUI)       │");
    println!("  └─────────────────────────────────────────┘");
    println!();
    println!("  Press Enter at any prompt to accept the default.");
    println!("  Passwords are hidden as you type.");
    println!();

    // Username
    print!("  Username [admin]: ");
    io::stdout().flush().ok();
    let mut raw = String::new();
    io::stdin().read_line(&mut raw)?;
    let uname = {
        let t = raw.trim();
        if t.is_empty() {
            "admin".to_string()
        } else {
            t.to_lowercase()
        }
    };
    agentum_core::validate_username(&uname).map_err(|e| anyhow!("invalid username: {e}"))?;

    // Loop until we get a valid (≥8 char) password that matches its
    // confirmation. Reprompting beats bailing out — the user already
    // committed to running the wizard, and a fresh install with no
    // admin account is a worse failure mode than a few retries.
    let pw = loop {
        let p = read_password_hidden("  Password (min 8 chars): ")?;
        if p.len() < 8 {
            println!("  ✖ Password must be at least 8 characters. Try again.");
            continue;
        }
        let c = read_password_hidden("  Confirm password:        ")?;
        if p != c {
            println!("  ✖ Passwords don't match. Try again.");
            continue;
        }
        break p;
    };

    println!();
    println!("  Creating account...");

    let hash = agentum_server::auth::hash_password(pw)
        .await
        .map_err(|e| anyhow!("hash failed: {e}"))?;
    let user = store.create_user(&uname, &hash).await?;

    println!();
    println!("  ✓ Admin account created.");
    println!("    Username: {}", user.username);
    println!("    Save your password — you'll need it for the dashboard and TUI.");
    println!();

    Ok(())
}

fn prompt_password(label: &str) -> Result<String> {
    use anyhow::Context;
    eprint!("{label}: ");
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("reading password from stdin")?;
    Ok(buf.trim_end_matches(['\n', '\r']).to_string())
}

fn read_password_hidden(prompt: &str) -> Result<String> {
    // Print to stdout (not stderr) so it stays in order with the
    // wizard's other output and gets flushed before rpassword
    // takes control of the terminal.
    print!("{prompt}");
    io::stdout().flush().ok();
    match rpassword::read_password() {
        Ok(pw) => Ok(pw),
        Err(_) => {
            // Fallback for non-TTY environments (pipes, CI).
            let mut buf = String::new();
            io::stdin().read_line(&mut buf)?;
            Ok(buf.trim_end_matches(['\n', '\r']).to_string())
        }
    }
}
