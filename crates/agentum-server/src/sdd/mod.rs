//! Agentum-native SDD service: safe artifact publication and isolated
//! authoritative worktrees. HTTP orchestration lives in `routes::sdd`.

pub mod artifacts;
pub mod credentials;
pub mod delivery;
pub mod evidence;
pub mod jira;
pub mod lifecycle;
pub mod provider_conformance;
pub mod providers;
pub mod remote;
pub mod remote_lifecycle;
pub mod remote_worker;
pub mod sources;
pub mod workspace;

use sha2::{Digest, Sha256};

pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
