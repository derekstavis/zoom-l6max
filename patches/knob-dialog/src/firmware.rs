//! Shared L6max firmware APIs plus knob-dialog's coefficient tables.
include!("../../api/l6max/mod.rs");

// Original EQ coefficient tables, not reconstructed curves.
pub const EQ_GAIN_DB_TABLE: u32 = 0x800a_5138;
pub const EQ_MID_FREQUENCY_HZ_TABLE: u32 = 0x800a_5738;
