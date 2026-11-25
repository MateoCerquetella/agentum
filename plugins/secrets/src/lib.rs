//! Secrets: a thread's `.env` (in its working directory). The tab lists key names and takes new
//! values in a masked field; a value goes browser → hub → agentd → the file, never into a thread or
//! a log. Agents are told to ask for a key here instead of asking for its value in chat.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "secrets",
    name: "Secrets",
    description: "A project's .env: add, update or delete keys without showing values; agents ask for keys here, not in chat.",
    icon: "lock",
    group: Group::Code,
    default_on: true,
    settings: &[],
};

/// What every agent is told, for a thread working in `dir`.
pub fn instructions(dir: &str) -> String {
    format!(
        "# Secrets\nWhen the work needs a credential (an API key, a token, a password), never ask for its value in chat, and never print, log or commit one. \
         Ask the person to add it in this thread's Secrets tab (in the side panel), naming each KEY and what it is for; it is saved to {dir}/.env, which git ignores. \
         Then load it from .env (a dotenv library, or `set -a; . ./.env; set +a` before a command) without showing the file."
    )
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
