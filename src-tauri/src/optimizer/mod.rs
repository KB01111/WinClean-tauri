//! WinClean 2.0 optimizer: services, EcoQoS, power, hardware, capabilities.
//! Safety policy lives in `policy` and is enforced by the command layer.

pub mod capabilities;
pub mod hardware;
pub mod policy;
pub mod power;
pub mod processes;
pub mod services;

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
