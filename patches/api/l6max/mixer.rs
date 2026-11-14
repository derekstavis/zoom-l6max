// L6MAX MAIN 1.10 mixer bindings.

firmware_functions! {
    pub unsafe fn mixer_get_aux(channel: u32, bus: u32) -> u32, at 0x8000_b080;
    pub unsafe fn mixer_get_level(channel: u32) -> u32, at 0x8000_b0b0;
    pub unsafe fn mixer_get_mute(channel: u32) -> u32, at 0x8000_b0c0;
    pub unsafe fn mixer_get_efx(channel: u32, bus: u32) -> u32, at 0x8000_b0f0;
    /// Returns an EFX parameter by effect type and parameter index.
    pub unsafe fn mixer_get_efx_param(effect_type: u32, index: u32) -> u32, at 0x8000_b240;
    pub unsafe fn mixer_get_eq_frequency(channel: u32, band: u32) -> u32, at 0x8000_b120;
    pub unsafe fn mixer_get_eq_gain(channel: u32, band: u32) -> u32, at 0x8000_b140;
    pub unsafe fn mixer_get_pan(channel: u32) -> u32, at 0x8000_b1d0;
    pub unsafe fn mixer_get_channel_flag(slot: u32) -> u32, at 0x8000_b190;
    pub unsafe fn mixer_get_compressor() -> u32, at 0x8000_b1c0;
    pub unsafe fn mixer_get_efx_type(bus: u32) -> u32, at 0x8000_b270;
    pub unsafe fn mixer_get_submix(channel: u32) -> u32, at 0x8000_b2a0;
    /// `fn(mapped_channel: u32, value: u8)`; integer arguments use the C ABI word registers.
    pub unsafe fn mixer_set_mute(channel: u32, value: u32) at 0x8000_bb90;
    /// Writes the slot flag and its mirrored UI state (supported slots: 4-7, 8-11).
    pub unsafe fn mixer_set_channel_flag(slot: u32, value: u32) at 0x8000_bc50;
    /// Writes the selected effect type for a bus; observed selector calls use bus 0.
    pub unsafe fn mixer_set_efx_type(bus: u32, effect_type: u32) at 0x8000_bd78;
    /// Applies the requested effect type and its stored parameter values.
    /// Returns zero when the type is already selected and no refresh is forced.
    pub unsafe fn mixer_apply_effect_type(effect_type: u32, force_refresh: u32) -> u32, at 0x8005_4340;
    /// Applies the currently stored mixer effect type and refreshes its controls.
    pub unsafe fn mixer_apply_effect_type_from_mixer(force_refresh: u32) at 0x8008_8dd8;
    pub unsafe fn mixer_apply_channel_parameter(channel: u32, mode: u32, absolute: u32, delta: i32, value: u32) at 0x8005_3b28;
    pub unsafe fn mixer_channel_index(channel: u32) -> u32, at 0x8005_64b0;
}
