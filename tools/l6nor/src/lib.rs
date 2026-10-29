//! Read and write L6/L6max MAIN NOR and related simulator device state.
//!
//! - [`nor`]: the 2 MiB MAIN NOR image, QEMU-identical seeding, version
//!   records and update marker; L6 resource-NOR seeding.
//! - [`bank`]: the firmware's two-slot settings-bank protocol.
//! - [`device`]: a state's NOR images and where its settings banks live.
//! - [`catalog`]: revision-scoped settings-bank layouts with code fingerprints.
//! - [`settings`]: a bank payload addressed by catalog field paths.
//! - [`rtc`]: the panel RTC backup domain (date/time and calendar validity).
//! - [`state`]: declarative state specs applied to a state directory.

pub mod bank;
pub mod catalog;
pub mod device;
pub mod nor;
pub mod rtc;
pub mod settings;
pub mod state;

pub use catalog::Catalog;
pub use device::Device;
pub use nor::MainNor;
pub use rtc::{DateTime, PanelRtc};
pub use settings::{FieldValue, Settings};
pub use state::StateSpec;
