//! Offline research adapter: use the production resolver, never duplicate its defaults.
use std::{collections::BTreeSet, io::Read};

use corral_core::{agent::AgentRole, config::agent_capabilities::AgentCapabilities};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let request: Value = serde_json::from_str(&input)?;
    let config: AgentCapabilities = serde_json::from_value(request["config"].clone())?;
    let supported: BTreeSet<String> = serde_json::from_value(request["supported"].clone())?;
    let mut snapshots = Vec::new();
    for role in [
        AgentRole::Worker,
        AgentRole::Reviewer,
        AgentRole::Pm,
        AgentRole::Planner,
        AgentRole::PlanReviewer,
    ] {
        let snapshot = config.resolve_for(role, Some("baseline"), Some(&supported));
        snapshots.push(json!(snapshot));
    }
    println!("{}", serde_json::to_string_pretty(&snapshots)?);
    Ok(())
}
