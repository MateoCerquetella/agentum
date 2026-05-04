use anyhow::{Context, Result};

use crate::cli::AuthCmd;

pub async fn run(action: AuthCmd) -> Result<()> {
    match action {
        AuthCmd::Show => {
            let token = agentum_server::auth::ensure_token()
                .context("could not read or generate auth token")?;
            let path = agentum_server::auth::token_path()
                .context("could not resolve auth token path")?;
            println!("{token}");
            eprintln!("(stored at {})", path.display());
            Ok(())
        }
        AuthCmd::Rotate => {
            let token = agentum_server::auth::rotate_token()
                .context("could not rotate auth token")?;
            let path = agentum_server::auth::token_path()
                .context("could not resolve auth token path")?;
            println!("{token}");
            eprintln!("(written to {})", path.display());
            eprintln!("note: any running `agentum serve` reloads on the next request");
            Ok(())
        }
    }
}
