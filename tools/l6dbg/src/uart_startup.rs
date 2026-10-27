//! Traces L6 panel-UART startup and input dispatch alongside DIN-UART and MIDI
//! receive-task initialization.
use l6max_diagnostics::gdb;
use l6max_host::{engine::Options, headless::Headless};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

const PANEL_USART1_IRQ: (u32, &str) = (0x080012a8, "panel_usart1_irq");
const PANEL_RX_PARSER: (u32, &str) = (0x08000544, "panel_uart_rx_parser");
const PANEL_RX_PARSER_RETURN: (u32, &str) = (0x080011e2, "panel_uart_rx_parser_return");
const RT1041_SAI_TCSR: u32 = 0x08;
const RT1041_SAI_TCR2: u32 = 0x10;
const RT1041_SAI_RCSR: u32 = 0x88;
const RT1041_SAI_RCR2: u32 = 0x90;
const RT1041_SAI_RCR5: u32 = 0x9c;
const TABLE_TASK_ENTRIES: [(u32, &str); 31] = [
    (0x80010938, "l6_audio_process_task_entry"),
    (0x80010228, "audio_float_ip_task_entry"),
    (0x80010390, "audio_int_ip_task_entry"),
    (0x8006d470, "event_port_task_entry"),
    (0x8001de58, "cpu_adc_task_entry"),
    (0x80039380, "sub_mcu_receive_task_entry"),
    (0x80019d90, "audio_time_lag_task_entry"),
    (0x80026260, "audio_efx_queue_task_entry"),
    (0x80037380, "stream_file_access_task_entry"),
    (0x80037bc0, "stream_preload_task_entry"),
    (0x80039340, "stream_update_record_task_entry"),
    (0x80038e48, "stream_update_play_task_entry"),
    (0x80036118, "sampler_file_access_task_entry"),
    (0x80036c60, "sampler_update_play_task_entry"),
    (0x8002b140, "level_meter_led_task_entry"),
    (0x80034608, "power_led_task_entry"),
    (0x80037288, "sound_pad_led_task_entry"),
    (0x800452d0, "usb_midi_send_task_entry"),
    (0x8003b908, "uart_midi_send_task_entry"),
    (0x8003bbb0, "ui_cycle_task_entry"),
    (0x8002f210, "midi_clock_task_entry"),
    (0x8001a7f0, "audio_guide_task_entry"),
    (0x8006dd70, "main_task_entry"),
    (0x80034bc8, "record_play_control_task_entry"),
    (0x8002b6c0, "mixer_parameter_save_task_entry"),
    (0x80036290, "sampler_file_assign_task_entry"),
    (0x80026dd0, "jig_checksum_task_entry"),
    (0x8001e4d0, "debug_console_task_entry"),
    (0x8001e4f0, "debug_log_task_entry"),
    (0x80046d50, "idle_task_entry"),
    (0x8006dd08, "timer_service_task_entry"),
];
const AUDIO_PATH_BREAKPOINTS: [(u32, &str); 4] = [
    (0x800335f8, "l6_sai1_rx_dma_callback"),
    (0x800180f8, "l6_audio_input_block_semaphore_callback"),
    (0x80012388, "l6_audio_staging_ring_producer"),
    (0x80010698, "l6_audio_process_block_consumer"),
];

fn encoder_report_counts(trace: &str) -> [usize; 6] {
    let bytes: Vec<_> = trace
        .lines()
        .filter_map(|line| {
            line.strip_prefix("l6 panel USART1 TX ")
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        })
        .collect();
    let mut counts = [0; 6];
    for pair in bytes.windows(2) {
        if pair[0] == 0xa1 && pair[1] < 6 {
            counts[pair[1] as usize] += 1;
        }
    }
    counts
}

const BREAKPOINTS: [(u32, &str); 114] = [
    (0x80067a60, "task_creation_startup_entry"),
    (0x80048248, "task_descriptor_table_create"),
    (0x80076c60, "freertos_xTaskCreate"),
    (0x800745b8, "scheduler_startup"),
    (0x8006dd72, "main_task_external_setup_call"),
    (0x8006dd76, "main_task_after_external_setup"),
    (0x8006dd7a, "main_task_after_media_setup"),
    (0x8006dd7e, "main_task_after_audio_setup"),
    (0x8006dd82, "main_task_before_din_init"),
    (0x8006dd86, "main_task_after_din_init"),
    (0x8001b0e8, "main_task_device_initialization"),
    (0x8001b194, "panel_startup_helper_address_8001b194"),
    (0x8001b198, "panel_startup_helper_address_8001b198"),
    (0x8001b208, "main_task_panel_ready_path"),
    (0x8001b2f8, "main_task_device_initialization_return"),
    (0x800071d8, "panel_ready_3v_setup"),
    (0x800344a8, "panel_ready_power_setup"),
    (0x800340e8, "panel_ready_sd_rail_setup"),
    (0x80033f70, "panel_ready_va_setup"),
    (0x8005f0d0, "panel_ready_audio_setup"),
    (0x800086b0, "panel_ready_device_setup"),
    (0x8003b478, "panel_ready_ui_setup"),
    (0x800026a0, "panel_ready_storage_setup"),
    (0x80006248, "panel_ready_media_state_check"),
    (0x80002490, "panel_ready_media_init"),
    (0x8000ee08, "panel_ready_mixer_init"),
    (0x800043f8, "panel_ready_log_init"),
    (0x8001b29e, "panel_ready_startup_tail_call"),
    (0x8000ad48, "panel_ready_callback_registration"),
    (0x8001b288, "panel_ready_after_media_state_check"),
    (0x8001b290, "panel_ready_before_mixer_init"),
    (0x8001b294, "panel_ready_after_mixer_init"),
    (0x80011920, "audio_init_sample_rate_setup"),
    (0x800103d8, "audio_init_float_path"),
    (0x80010678, "audio_init_integer_path"),
    (0x80019fb8, "audio_init_time_lag"),
    (0x8000cfd0, "audio_init_clock_setup"),
    (0x8000cf48, "audio_init_adc_setup"),
    (0x80033768, "audio_init_driver_setup"),
    (0x80008608, "audio_init_gpio_setup"),
    (0x8001a828, "audio_init_adc_powerdown"),
    (0x800337a0, "audio_init_codec_setup"),
    (0x8001a830, "audio_init_sample_rate_apply"),
    (0x80033778, "audio_init_stream_setup"),
    (0x8000e9b0, "audio_init_process_setup"),
    (0x800263b0, "audio_init_callback_setup"),
    (0x8000e600, "audio_init_mixer_setup"),
    (0x8000275a, "audio_init_after_mixer_setup"),
    (0x80002768, "audio_init_after_dac_mute_gpio"),
    (0x8000277a, "audio_init_after_dac_mute_log"),
    (0x8000278c, "audio_init_after_amute_gpio"),
    (0x800027b2, "audio_init_return_tail_call"),
    (0x8001b284, "main_task_after_audio_initializer"),
    (0x8000e61c, "audio_mixer_lock_wait"),
    (0x8000e620, "audio_mixer_lock_return"),
    (0x8000e6a6, "audio_mixer_before_stream_activation"),
    (0x8000e6ac, "audio_mixer_after_stream_activation"),
    (0x8000e6b0, "audio_mixer_before_engine_start"),
    (0x8000e6b4, "audio_mixer_after_engine_start"),
    (0x8000e6b8, "audio_mixer_before_first_callback"),
    (0x8000e6d2, "audio_mixer_first_callback_call"),
    (0x8000e6d4, "audio_mixer_after_first_callback"),
    (0x8000e6d8, "audio_mixer_after_device_setup"),
    (0x8000e6dc, "audio_mixer_after_engine_commit"),
    (0x8000e6e8, "audio_mixer_before_second_callback"),
    (0x8000e700, "audio_mixer_second_callback_call"),
    (0x8000e704, "audio_mixer_after_second_callback"),
    (0x8000e70a, "audio_mixer_after_context_release"),
    (0x8000e70e, "audio_mixer_before_lock_release"),
    (0x800258e8, "audio_mixer_profile_callback_entry"),
    (0x80025900, "audio_mixer_profile_first_dispatch_call"),
    (0x80025902, "audio_mixer_profile_first_dispatch_return"),
    (0x80025910, "audio_mixer_profile_second_dispatch_return"),
    (0x8002592a, "audio_mixer_profile_third_dispatch_return"),
    (0x8002593c, "audio_mixer_profile_fourth_dispatch_return"),
    (0x80025950, "audio_mixer_profile_fifth_dispatch_return"),
    (0x80025960, "audio_mixer_profile_sixth_dispatch_return"),
    (0x80025974, "audio_mixer_profile_seventh_dispatch_return"),
    (0x80025984, "audio_mixer_profile_eighth_dispatch_return"),
    (0x80026740, "audio_profile_third_dispatch_target"),
    (0x8002676e, "audio_profile_write_target"),
    (0x800268e0, "audio_profile_fourth_dispatch_target"),
    (0x80026974, "audio_profile_second_lock_set"),
    (0x80026976, "audio_profile_second_lock_poll"),
    (0x80026982, "audio_profile_second_lock_recheck"),
    (0x8004fa98, "audio_profile_dsp_stream_callback"),
    (0x80026776, "audio_profile_lock_owner_store"),
    (0x80026782, "audio_profile_lock_set"),
    (0x80026784, "audio_profile_lock_poll"),
    (0x80026786, "audio_profile_lock_wait_branch"),
    (0x80026788, "audio_profile_lock_wait_call"),
    (0x8002678e, "audio_profile_lock_wait_recheck"),
    (0x80026792, "audio_profile_lock_wait_repeat"),
    (0x8002599e, "audio_mixer_profile_tailcall_a"),
    (0x80025a00, "audio_mixer_profile_tailcall_b"),
    (0x80039a68, "panel_service_entry"),
    (0x8002d858, "midi_control_task_entry"),
    (0x8002e7a8, "midi_setting_receive_task_entry"),
    (0x8002e8c8, "din_receive_task_entry"),
    (0x8002f8a0, "din_uart_init"),
    (0x8002f490, "midi_message_dispatch"),
    (0x800337b8, "uart_init_wrapper"),
    (0x80029e38, "uart_register_setup"),
    (0x80029bb8, "lpuart7_irq_handler"),
    (0x8003a080, "uart6_receive_callback"),
    (0x80039ef8, "panel_uart_event_callback"),
    (0x800393d0, "panel_event_dispatch"),
    (0x80039af4, "panel_update_probe"),
    (0x80039cb0, "panel_service_loop_start"),
    (0x80039d5a, "panel_event_callback_installed"),
    (0x8003a338, "panel_update_handshake_wait"),
    (0x80039af8, "panel_update_handshake_return"),
    (0x8002e95e, "din_receive_ring_read"),
    (0x80001414, "reset_handler"),
];

pub fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let mut connect_midi = false;
    let mut exercise_inputs = false;
    let mut force_panel_parser = false;
    let mut stop_after_handshake = false;
    let mut stop_after_mixer_lock = false;
    let mut stop_after_mixer_callback_dispatch = false;
    let mut step_audio_profile_write = false;
    let mut release_audio_profile_lock = false;
    let mut startup_only = false;
    let mut trace_audio_path = false;
    let mut engine_args = Vec::new();
    let mut explicit_state_dir = false;
    for argument in arguments {
        if argument == "--connect-midi" {
            connect_midi = true;
        } else if argument == "--exercise-inputs" {
            exercise_inputs = true;
        } else if argument == "--force-panel-parser" {
            force_panel_parser = true;
        } else if argument == "--stop-after-handshake" {
            stop_after_handshake = true;
        } else if argument == "--stop-after-mixer-lock" {
            stop_after_mixer_lock = true;
        } else if argument == "--stop-after-mixer-callback-dispatch" {
            stop_after_mixer_callback_dispatch = true;
        } else if argument == "--step-audio-profile-write" {
            step_audio_profile_write = true;
        } else if argument == "--release-audio-profile-lock" {
            release_audio_profile_lock = true;
        } else if argument == "--startup-only" {
            startup_only = true;
        } else if argument == "--trace-audio-path" {
            trace_audio_path = true;
        } else {
            explicit_state_dir |= argument == "--state-dir";
            engine_args.push(argument);
        }
    }
    let mut options = Options::parse_args(engine_args)?;
    if force_panel_parser && !exercise_inputs {
        return Err("--force-panel-parser requires --exercise-inputs".into());
    }
    if options.state_dir.is_some() && !explicit_state_dir {
        return Err("use --volatile or provide an isolated --state-dir".into());
    }
    options.usb_enabled = false;
    options.midi_enabled = connect_midi;
    options.start_paused = true;
    let socket = PathBuf::from(format!(
        "l6dbg-uart-startup-{}.gdb.sock",
        std::process::id()
    ));
    let socket = std::env::temp_dir().join(socket);
    options.gdb_socket = Some(socket.clone());

    let guest = Headless::start(&options)?;
    if guest.engine.model() != l6fw::Model::L6 {
        return Err("l6dbg uart-startup requires L6 firmware".into());
    }
    let qmp = &guest.engine.management[0];
    qmp.stop()?;
    let mut debugger = gdb::Gdb::connect(&socket)?;
    for (address, _) in BREAKPOINTS {
        debugger.breakpoint(address, true)?;
    }
    for (address, _) in TABLE_TASK_ENTRIES {
        debugger.breakpoint(address, true)?;
    }
    if trace_audio_path {
        for (address, _) in AUDIO_PATH_BREAKPOINTS {
            debugger.breakpoint(address, true)?;
        }
    }
    qmp.resume()?;

    let deadline = Instant::now() + Duration::from_secs(if trace_audio_path { 240 } else { 120 });
    let mut hits = Vec::new();
    let mut inputs_exercised = false;
    let mut encoder_inputs_sent = 0;
    let mut encoder_dispatches = 0;
    let mut encoder_indicator_before: Vec<[u32; 8]> = Vec::new();
    let mut encoder_indicator_changes = Vec::new();
    let mut panel_parser_forced = false;
    let mut handshake_parser_seen = false;
    let mut mixer_lock_returned = false;
    let mut mixer_callback_dispatch_seen = false;
    let mut mixer_callback_context = None;
    let mut audio_profile_write_step = None;
    let mut audio_profile_lock_released = false;
    let mut panel_rx_ring = None;
    let mut panel_rx_bytes = None;
    let mut panel_parser_buffer = None;
    let mut panel_parser_state = None;
    while guest.guest_ms() < 60000
        && !(startup_only && guest.guest_ms() >= 1500)
        && !(stop_after_handshake && handshake_parser_seen)
        && !(stop_after_mixer_lock && mixer_lock_returned)
        && !(stop_after_mixer_callback_dispatch && mixer_callback_dispatch_seen)
    {
        if !qmp.query_status()?.running {
            let pc = debugger.read_register(15)? & !1;
            let Some((address, name)) = BREAKPOINTS
                .iter()
                .find(|(address, _)| *address == pc)
                .or_else(|| {
                    TABLE_TASK_ENTRIES
                        .iter()
                        .find(|(address, _)| *address == pc)
                })
                .or_else(|| {
                    trace_audio_path
                        .then(|| {
                            AUDIO_PATH_BREAKPOINTS
                                .iter()
                                .find(|(address, _)| *address == pc)
                        })
                        .flatten()
                })
                .or_else(|| (PANEL_USART1_IRQ.0 == pc).then_some(&PANEL_USART1_IRQ))
                .or_else(|| (PANEL_RX_PARSER.0 == pc).then_some(&PANEL_RX_PARSER))
                .or_else(|| (PANEL_RX_PARSER_RETURN.0 == pc).then_some(&PANEL_RX_PARSER_RETURN))
            else {
                return Err(format!("guest stopped at unexpected PC {pc:#010x}").into());
            };
            let registers = (0..4)
                .map(|register| debugger.read_register(register))
                .collect::<Result<Vec<_>, _>>()?;
            let callee_registers = if *address == 0x80026740 {
                Some(
                    (4..12)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            } else {
                None
            };
            let link_register = debugger.read_register(14)?;
            let audio_profile_lock = if (0x80026776..=0x80026792).contains(address)
                || (0x80026974..=0x80026986).contains(address)
            {
                qmp.read_memory(0, 0x801f8cd4, 4).ok()
            } else {
                None
            };
            let audio_path_state = if trace_audio_path
                && AUDIO_PATH_BREAKPOINTS
                    .iter()
                    .any(|(audio_address, _)| audio_address == address)
            {
                let rx_tcd = qmp.read_memory(0, 0x400e91e0, 0x20).ok();
                let rx_destination = rx_tcd
                    .as_ref()
                    .and_then(|tcd| tcd.get(16..20))
                    .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()));
                let rx_destination_bytes =
                    rx_destination.and_then(|destination| qmp.read_memory(0, destination, 32).ok());
                Some(json!({
                    "handler": qmp.read_memory(0, 0x801f9158, 8).ok(),
                    "dma_channel15_tcd": rx_tcd,
                    "dma_channel15_destination": rx_destination,
                    "dma_channel15_destination_bytes": rx_destination_bytes,
                    "sai1_rx_registers": {
                        "rcsr": qmp.read_memory(0, 0x40384088, 4).ok(),
                        "rcr3": qmp.read_memory(0, 0x40384094, 4).ok(),
                        "rcr4": qmp.read_memory(0, 0x40384098, 4).ok(),
                        "rfr0_rfr3": qmp.read_memory(0, 0x403840c0, 16).ok(),
                    },
                    "firmware_rx_buffers": {
                        "buffer_0": qmp.read_memory(0, 0x2000dce0, 32).ok(),
                        "buffer_1": qmp.read_memory(0, 0x2000ece0, 32).ok(),
                    },
                    "staging_buffer_head": qmp.read_memory(0, 0x2000f5c0, 32).ok(),
                    "ring_indices_and_flags": qmp.read_memory(0, 0x2000bb08, 0x0c).ok(),
                    "current_ring_descriptor": qmp.read_memory(0, 0x2000f9c0, 8).ok(),
                }))
            } else {
                None
            };
            let panel_event_context = if *address == 0x800393d0 {
                Some(json!({
                    "report": qmp.read_memory(0, 0x808cb54d, 8).ok(),
                    "r2": format!("{:#010x}", registers[2]),
                    "r2_bytes": qmp.read_memory(0, registers[2], 32).ok(),
                    "r3": format!("{:#010x}", registers[3]),
                    "r3_bytes": qmp.read_memory(0, registers[3], 32).ok(),
                }))
            } else {
                None
            };
            hits.push(json!({
                "function": name,
                "address": format!("{address:#010x}"),
                "r0_r3": registers,
                "r4_r11": callee_registers,
                "audio_profile_lock": audio_profile_lock,
                "audio_path_state": audio_path_state,
                "panel_event_context": panel_event_context,
                "lr": format!("{link_register:#010x}"),
                "guest_ms": guest.guest_ms(),
            }));
            if *address == 0x8000e620 {
                mixer_lock_returned = true;
            }
            if *address == 0x800258e8 {
                let context = debugger.read_register(1)?;
                mixer_callback_context = qmp.read_memory(0, context, 0xa0).ok().map(|bytes| {
                    json!({
                        "address": format!("{context:#010x}"),
                        "bytes": bytes,
                    })
                });
            }
            if *address == 0x8002599e || *address == 0x80025a00 || *address == 0x80026740 {
                mixer_callback_dispatch_seen = true;
            }
            if release_audio_profile_lock && (*address == 0x80026784 || *address == 0x80026976) {
                debugger.write(0x801f8cd4, &0u32.to_le_bytes())?;
                audio_profile_lock_released = true;
            }
            let stepped_audio_write = step_audio_profile_write && *address == 0x8002676e;
            if stepped_audio_write {
                let code_before = qmp.read_memory(0, *address, 12).ok();
                debugger.breakpoint(*address, false)?;
                debugger.breakpoint(0x80026776, false)?;
                let mut steps = Vec::new();
                let mut destination = None;
                let mut destination_before = None;
                for step in 0..3 {
                    let stop = debugger.request("s")?;
                    if step == 0 {
                        // The third instruction is `str r0, [r4, #0x0c]`;
                        // the two MOVT instructions before it construct r4.
                        let address = debugger.read_register(4)? + 0x0c;
                        destination = Some(address);
                        destination_before = qmp.read_memory(0, address, 4).ok();
                    }
                    steps.push(json!({
                        "stop": stop,
                        "pc": format!("{:#010x}", debugger.read_register(15)? & !1),
                    }));
                }
                let destination = destination.ok_or("missing audio profile write target")?;
                audio_profile_write_step = Some(json!({
                    "code_before": code_before,
                    "destination": format!("{destination:#010x}"),
                    "destination_before": destination_before,
                    "destination_after": qmp.read_memory(0, destination, 4).ok(),
                    "steps": steps,
                    "r0_r12": (0..13)
                        .map(|register| debugger.read_register(register))
                        .collect::<Result<Vec<_>, _>>()?,
                }));
            }
            if *address == PANEL_RX_PARSER.0 {
                debugger.breakpoint(PANEL_RX_PARSER_RETURN.0, true)?;
            }
            if *address == PANEL_RX_PARSER_RETURN.0 {
                handshake_parser_seen = true;
                panel_rx_ring = qmp.read_memory(1, 0x200000e8, 32).ok();
                panel_rx_bytes = qmp.read_memory(1, 0x2000030c, 16).ok();
                panel_parser_buffer = qmp.read_memory(1, 0x20000514, 16).ok();
                panel_parser_state = qmp.read_memory(1, 0x2000050c, 12).ok();
            }
            if force_panel_parser && *address == 0x80039af8 && !panel_parser_forced {
                // Diagnostic only: bypass the failed startup exchange and
                // exercise the firmware's normal parser in volatile RAM.
                debugger.write(0x808cb63c, &0x80039ef9u32.to_le_bytes())?;
                panel_parser_forced = true;
            }
            if *address == 0x8003a338 {
                debugger.breakpoint(PANEL_USART1_IRQ.0, true)?;
            }
            if *address == PANEL_USART1_IRQ.0 {
                debugger.breakpoint(PANEL_RX_PARSER.0, true)?;
            }
            if stepped_audio_write {
                break;
            }
            debugger.breakpoint(*address, false)?;
            if encoder_inputs_sent > 0 && guest.guest_ms() < 52000 && *address == 0x800393d0 {
                encoder_dispatches += 1;
            }
            qmp.resume()?;
        } else {
            if exercise_inputs
                && encoder_inputs_sent < 6
                && guest.guest_ms() >= 40000 + encoder_inputs_sent as u32 * 2000
            {
                qmp.stop()?;
                let inputs = guest.engine.inputs.as_ref().ok_or("QEMU required")?;
                if encoder_inputs_sent > 0 {
                    let channel = (encoder_inputs_sent - 1) as usize;
                    let after = inputs.panel.snapshot().indicators;
                    let before = encoder_indicator_before[channel];
                    encoder_indicator_changes.push(json!({
                        "channel": channel,
                        "before": before,
                        "after": after,
                        "changed": std::array::from_fn::<_, 8, _>(|row| before[row] ^ after[row]),
                    }));
                }
                for address in [0x80029bb8, 0x8003a080, 0x80039ef8, 0x800393d0] {
                    debugger.breakpoint(address, true)?;
                }
                encoder_indicator_before.push(inputs.panel.snapshot().indicators);
                inputs.panel.encoder(encoder_inputs_sent as u32, 1)?;
                qmp.resume()?;
                encoder_inputs_sent += 1;
            } else if exercise_inputs
                && encoder_inputs_sent == 6
                && !inputs_exercised
                && guest.guest_ms() >= 52000
            {
                qmp.stop()?;
                let inputs = guest.engine.inputs.as_ref().ok_or("QEMU required")?;
                let channel = (encoder_inputs_sent - 1) as usize;
                let before = encoder_indicator_before[channel];
                let after = inputs.panel.snapshot().indicators;
                encoder_indicator_changes.push(json!({
                    "channel": channel,
                    "before": before,
                    "after": after,
                    "changed": std::array::from_fn::<_, 8, _>(|row| before[row] ^ after[row]),
                }));
                for target in [1, 7, 47] {
                    inputs.panel.tap(target, 120)?;
                }
                if !force_panel_parser {
                    inputs.main.tap(49, 120)?;
                }
                qmp.resume()?;
                inputs_exercised = true;
            }
            guest.sample_display()?;
            if Instant::now() > deadline {
                return Err("60-second guest probe timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    qmp.stop()?;

    let trace = fs::read_to_string(options.logs.join("engine.trace"))?;
    let midi_config = qmp.read_memory(0, 0x804410f0, 0x3b).ok();
    let panel_flash_after_package = qmp
        .read_memory(1, 0x08003480, 16)
        .or_else(|_| qmp.read_memory(1, 0x00003480, 16))
        .ok();
    let panel_uart_callback = qmp
        .read_memory(0, 0x808cb63c, 4)
        .ok()
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
    let audio_mixer_setup_state = qmp.read_memory(0, 0x8053d198, 0x20).ok();
    let audio_mixer_profile_table = qmp.read_memory(0, 0x801f5c00, 0x20).ok();
    let audio_profile_lock_state = qmp.read_memory(0, 0x801f8cd4, 0x10).ok();
    let audio_dma_tcds = if trace_audio_path {
        Some(json!({
            "channel12": {
                "live": qmp.read_memory(0, 0x400e9180, 0x20).ok(),
                "scatter_gather": qmp.read_memory(0, 0x2000f540, 0x20).ok(),
            },
            "channel13": {
                "live": qmp.read_memory(0, 0x400e91a0, 0x20).ok(),
                "scatter_gather": qmp.read_memory(0, 0x2000f4e0, 0x20).ok(),
            },
            "channel14": {
                "live": qmp.read_memory(0, 0x400e91c0, 0x20).ok(),
                "scatter_gather": qmp.read_memory(0, 0x2000f500, 0x20).ok(),
            },
            "channel15": {
                "live": qmp.read_memory(0, 0x400e91e0, 0x20).ok(),
                "scatter_gather": qmp.read_memory(0, 0x2000f520, 0x20).ok(),
            },
        }))
    } else {
        None
    };
    let audio_sai_state = if trace_audio_path {
        Some(json!({
            "sai1": {
                "tcsr": qmp.read_memory(0, 0x40384000 + RT1041_SAI_TCSR, 4).ok(),
                "rcsr": qmp.read_memory(0, 0x40384000 + RT1041_SAI_RCSR, 4).ok(),
                "tcr2": qmp.read_memory(0, 0x40384000 + RT1041_SAI_TCR2, 4).ok(),
                "rcr2": qmp.read_memory(0, 0x40384000 + RT1041_SAI_RCR2, 4).ok(),
                "tcr3": qmp.read_memory(0, 0x40384014, 4).ok(),
                "tcr4": qmp.read_memory(0, 0x40384018, 4).ok(),
                "tcr1": qmp.read_memory(0, 0x4038400c, 4).ok(),
                "rcr3": qmp.read_memory(0, 0x40384094, 4).ok(),
                "rcr4": qmp.read_memory(0, 0x40384098, 4).ok(),
                "rcr5": qmp.read_memory(0, 0x40384000 + RT1041_SAI_RCR5, 4).ok(),
                "rcr1": qmp.read_memory(0, 0x4038408c, 4).ok(),
            },
            "sai3": {
                "tcsr": qmp.read_memory(0, 0x4038c000 + RT1041_SAI_TCSR, 4).ok(),
                "rcsr": qmp.read_memory(0, 0x4038c000 + RT1041_SAI_RCSR, 4).ok(),
                "tcr2": qmp.read_memory(0, 0x4038c000 + RT1041_SAI_TCR2, 4).ok(),
                "rcr2": qmp.read_memory(0, 0x4038c000 + RT1041_SAI_RCR2, 4).ok(),
                "tcr3": qmp.read_memory(0, 0x4038c014, 4).ok(),
                "tcr4": qmp.read_memory(0, 0x4038c018, 4).ok(),
                "tcr1": qmp.read_memory(0, 0x4038c00c, 4).ok(),
                "rcr3": qmp.read_memory(0, 0x4038c094, 4).ok(),
                "rcr4": qmp.read_memory(0, 0x4038c098, 4).ok(),
                "rcr5": qmp.read_memory(0, 0x4038c000 + RT1041_SAI_RCR5, 4).ok(),
                "rcr1": qmp.read_memory(0, 0x4038c08c, 4).ok(),
            },
        }))
    } else {
        None
    };
    let report = json!({
        "model": "L6",
        "midi_connected": connect_midi,
        "inputs_exercised": inputs_exercised,
        "encoder_inputs_sent": encoder_inputs_sent,
        "encoder_dispatches": encoder_dispatches,
        "encoder_panel_reports": encoder_report_counts(&trace),
        "encoder_indicator_changes": encoder_indicator_changes,
        "panel_parser_forced": panel_parser_forced,
        "guest_ms": guest.guest_ms(),
        "mixer_lock_returned": mixer_lock_returned,
        "mixer_callback_dispatch_seen": mixer_callback_dispatch_seen,
        "mixer_callback_context": mixer_callback_context,
        "audio_profile_write_step": audio_profile_write_step,
        "audio_profile_lock_released": audio_profile_lock_released,
        "startup_only": startup_only,
        "audio_path_traced": trace_audio_path,
        "stopped_pc": format!("{:#010x}", debugger.read_register(15)? & !1),
        "breakpoint_hits": hits,
        "lpuart2_ctrl_writes": trace.matches("l6 main LPUART2 (MIDI jacks) CTRL ").count(),
        "lpuart2_host_rx": trace.matches("l6 main LPUART2 (MIDI jacks) host RX ").count(),
        "panel_uart_callback": panel_uart_callback.map(|address| format!("{address:#010x}")),
        "panel_flash_after_package": panel_flash_after_package,
        "audio_mixer_setup_state": audio_mixer_setup_state,
        "audio_mixer_profile_table": audio_mixer_profile_table,
        "audio_profile_lock_state": audio_profile_lock_state,
        "audio_dma_tcds": audio_dma_tcds,
        "audio_sai_state": audio_sai_state,
        "audio_registered_handler": trace_audio_path.then(|| qmp.read_memory(0, 0x801f9158, 8).ok()),
        "audio_ring_indices_and_flags": trace_audio_path.then(|| qmp.read_memory(0, 0x2000bb08, 0x0c).ok()),
        "panel_rx_ring": panel_rx_ring,
        "panel_rx_bytes": panel_rx_bytes,
        "panel_parser_buffer": panel_parser_buffer,
        "panel_parser_state": panel_parser_state,
        "midi_configuration_ram": midi_config.map(|bytes| json!({
            "configured_channel": bytes[0],
            "cc_map": bytes[5..].iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>(),
        })),
    });
    let path: PathBuf = options.logs.join("uart-startup.json");
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
