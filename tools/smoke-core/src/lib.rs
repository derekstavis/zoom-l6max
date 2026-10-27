//! Shared scenario definition, execution and evidence for headless and GPUI tests.
pub mod devices;
pub mod host_midi;
mod runner;
mod scenario;
mod session;
mod trace;
pub use runner::{Progress, Runner};
pub use scenario::{Action, Gesture, Scenario, Step, control_target, is_switch, model_name};
use serde::Serialize;
pub use session::{Command, Session, run};
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct CaseResult {
    pub case: String,
    pub model: String,
    pub backend: String,
    pub outcome: String,
    pub detail: String,
    pub artifacts: PathBuf,
}
