// L6 MAIN 1.10 mixer bindings.

firmware_functions! {
    pub unsafe fn mixer_get_eq_frequency(channel: u32, band: u32) -> u32, at 0x8000_5f90;
    pub unsafe fn mixer_get_eq_gain(channel: u32, band: u32) -> u32, at 0x8000_5fa8;
    pub unsafe fn mixer_get_aux(channel: u32, bus: u32) -> u32, at 0x8000_60e8;
    pub unsafe fn mixer_get_efx(channel: u32, bus: u32) -> u32, at 0x8000_5f60;
    pub unsafe fn mixer_get_pan(channel: u32) -> u32, at 0x8000_6028;
    pub unsafe fn mixer_get_level(channel: u32) -> u32, at 0x8000_5f40;
    pub unsafe fn mixer_channel_index(channel: u32) -> u32, at 0x8004_0ae8;
}
