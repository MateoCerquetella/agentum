//! `agentum profiles` — manage named connection profiles for the TUI.
//!
//! See `crates/agentum/src/commands/terminal/profiles.rs` for the
//! storage format. This module is a thin CLI shell over that store.

use anyhow::{Context, Result};

use crate::cli::ProfilesCmd;
use crate::commands::terminal::profiles::{Profile, Profiles};

pub async fn run(action: ProfilesCmd) -> Result<()> {
    match action {
        ProfilesCmd::List => list().await,
        ProfilesCmd::Add {
            name,
            url,
            fingerprint,
            insecure,
            set_default,
        } => add(name, url, fingerprint, insecure, set_default).await,
        ProfilesCmd::Rm { name } => remove(name).await,
        ProfilesCmd::Use { name, clear } => use_(name, clear).await,
    }
}

async fn list() -> Result<()> {
    let profiles = Profiles::load().context("load profiles.toml")?;
    let entries = profiles.list();
    if entries.is_empty() {
        eprintln!("no profiles defined ({})", profiles.path().display());
        eprintln!("add one with: agentum profiles add NAME https://host:8822");
        return Ok(());
    }
    // Plain-text columns (name, default-marker, url, extras) so the
    // output stays grep-able and pipeable. The marker is `*` on the
    // default profile so the "which is current?" answer reads at a
    // glance without ANSI colouring.
    let name_w = entries
        .iter()
        .map(|(n, _, _)| n.len())
        .max()
        .unwrap_or(8)
        .max(8);
    println!(
        "{:width$}  {:<3}  URL",
        "NAME",
        "DEF",
        width = name_w,
    );
    for (name, p, is_default) in entries {
        let mut suffix = String::new();
        if let Some(fp) = &p.fingerprint {
            suffix.push_str(&format!("  fingerprint={fp}"));
        }
        if p.insecure {
            suffix.push_str("  insecure");
        }
        println!(
            "{:width$}  {:<3}  {}{}",
            name,
            if is_default { "*" } else { "" },
            p.url,
            suffix,
            width = name_w,
        );
    }
    Ok(())
}

async fn add(
    name: String,
    url: String,
    fingerprint: Option<String>,
    insecure: bool,
    set_default: bool,
) -> Result<()> {
    // Validate the URL up front so a typo is caught at write time
    // instead of much later when the TUI tries to connect.
    url::Url::parse(&url).with_context(|| format!("invalid URL: {url}"))?;

    let mut profiles = Profiles::load().context("load profiles.toml")?;
    profiles
        .upsert(
            name.clone(),
            Profile {
                url,
                fingerprint,
                insecure,
            },
        )
        .with_context(|| format!("save profile `{name}`"))?;

    if set_default {
        profiles
            .set_default(Some(name.clone()))
            .with_context(|| format!("mark `{name}` default"))?;
    }
    println!("saved profile `{name}` to {}", profiles.path().display());
    // Drop a copy-pasteable next-step hint after every successful
    // save. The most common confusion is "I added a profile, now what?"
    // so naming the exact command here closes the loop.
    if set_default {
        println!("default profile is now `{name}` — run `agentum terminal` to connect.");
    } else {
        println!("connect with: agentum terminal --profile {name}");
    }
    Ok(())
}

async fn remove(name: String) -> Result<()> {
    let mut profiles = Profiles::load().context("load profiles.toml")?;
    if profiles.remove(&name)? {
        println!("removed profile `{name}`");
    } else {
        eprintln!("no profile named `{name}`");
    }
    Ok(())
}

async fn use_(name: Option<String>, clear: bool) -> Result<()> {
    let mut profiles = Profiles::load().context("load profiles.toml")?;
    if clear {
        profiles.set_default(None)?;
        println!("cleared default profile");
        return Ok(());
    }
    let Some(name) = name else {
        anyhow::bail!("`agentum profiles use` requires NAME or --clear");
    };
    profiles.set_default(Some(name.clone()))?;
    println!("default profile is now `{name}`");
    Ok(())
}
