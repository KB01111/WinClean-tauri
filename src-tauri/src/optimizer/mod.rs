//! WinClean 2.0 optimizer: services, EcoQoS, power, hardware, capabilities.
//! Safety policy lives in `policy` and is enforced by the command layer.

pub mod auto_switch;
pub mod backend;
pub mod battery;
pub mod capabilities;
pub mod commands;
pub mod ecoqos;
pub mod hardware;
pub mod helper_client;
pub mod helper_protocol;
pub mod helper_windows;
pub mod measure;
pub mod orchestrator;
pub mod policy;
pub mod power;
pub mod power_events;
pub mod privileged;
pub mod processes;
pub mod profiles;
pub mod recovery;
pub mod runtime;
pub mod service_graph;
pub mod service_plan;
pub mod services;
pub mod state;
pub mod telemetry;
pub mod workload;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationSnapshot {
    pub id: i64,
    pub kind: String,
    pub target: String,
    pub payload_json: String,
    pub created_at_ms: i64,
    pub rolled_back_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationAudit {
    pub id: i64,
    pub kind: String,
    pub target: String,
    pub action: String,
    pub status: String,
    pub detail: Option<String>,
    pub snapshot_id: Option<i64>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}
