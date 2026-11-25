//! Usage: Each agent's limits, tokens and cost by day and project.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "usage",
    name: "Usage",
    description: "Each agent's limits, tokens and cost by day and project.",
    icon: "bar-chart",
    group: Group::Usage,
    default_on: true,
    settings: &[],
};

/// Tokens and cost of some turns (the hub's sums, by hour and by agent, machine and project).
#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Stat {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub turns: u64,
}

impl Stat {
    /// Every token the turns moved (as the timeline counts them).
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
    pub fn merge(&mut self, o: &Stat) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cost += o.cost;
        self.turns += o.turns;
    }
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
