use std::io::{self, Write};

use anyhow::{Result, anyhow};

use crate::cli::AuthCmd;

pub async fn run(action: AuthCmd) -> Result<()> {
    let (store, _) = super::open_store().await?;

    match action {
        AuthCmd::List => {
            let users = store.list_users().await?;
            if users.is_empty() {
                println!("(no users — register from the dashboard on first visit)");
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
                None => prompt_password()?,
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
    }
}

fn prompt_password() -> Result<String> {
    use anyhow::Context;
    eprint!("password: ");
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("reading password from stdin")?;
    Ok(buf.trim_end_matches(['\n', '\r']).to_string())
}
