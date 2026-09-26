//! Hardware fixtures representing typical machines.
//!
//! These are explicit development/test fixtures: they drive the unit tests
//! and the "Simulate hardware" option in Advanced settings (debug builds
//! only). They are never used to describe the real machine.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::benchmark::BenchmarkSummary;
use super::capabilities::HardwareSnapshot;
use super::profile::CapabilityProfile;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HardwareFixture {
    pub id: String,
    pub description: String,
    pub hardware: HardwareSnapshot,
    /// Optional benchmark results to test promotion/demotion.
    #[serde(default)]
    pub benchmark: Option<BenchmarkSummary>,
    pub expected: FixtureExpectation,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FixtureExpectation {
    pub profile: CapabilityProfile,
    pub llm_model_id: Option<String>,
    /// Expected profile once `benchmark` is applied, when present.
    #[serde(default)]
    pub profile_after_benchmark: Option<CapabilityProfile>,
    #[serde(default)]
    pub llm_model_id_after_benchmark: Option<String>,
}

const FIXTURES: &[(&str, &str)] = &[
    ("win-intel-8gb", include_str!("../../fixtures/hardware/win-intel-8gb.json")),
    ("mac-m1-16gb", include_str!("../../fixtures/hardware/mac-m1-16gb.json")),
    ("win-cpu-16gb", include_str!("../../fixtures/hardware/win-cpu-16gb.json")),
    ("win-16gb-rtx-8gb", include_str!("../../fixtures/hardware/win-16gb-rtx-8gb.json")),
    ("mac-24gb", include_str!("../../fixtures/hardware/mac-24gb.json")),
    ("mac-32gb", include_str!("../../fixtures/hardware/mac-32gb.json")),
    ("win-32gb-rtx-12gb", include_str!("../../fixtures/hardware/win-32gb-rtx-12gb.json")),
    ("win-32gb-cpu-only", include_str!("../../fixtures/hardware/win-32gb-cpu-only.json")),
    ("mac-m4max-48gb", include_str!("../../fixtures/hardware/mac-m4max-48gb.json")),
];

pub fn all() -> Vec<HardwareFixture> {
    FIXTURES
        .iter()
        .map(|(id, json)| {
            serde_json::from_str::<HardwareFixture>(json).unwrap_or_else(|e| panic!("fixture {id} is invalid: {e}"))
        })
        .collect()
}

pub fn load(id: &str) -> HardwareFixture {
    all().into_iter().find(|f| f.id == id).unwrap_or_else(|| panic!("unknown fixture {id}"))
}

pub fn ids() -> Vec<String> {
    FIXTURES.iter().map(|(id, _)| id.to_string()).collect()
}
