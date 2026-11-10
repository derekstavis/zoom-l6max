// Hookable L6max MAIN 1.10 function entries and loader metadata.

define_function_hooks! {
    firmware_bindings;
    fs_mount {
        displaced_len: 4,
        guard_sha256: "cfa045938b57940b57930f97cd7cdf3e1a8c3b1ddb72c2609b95b97c1b1921ed",
        continuation: Linear,
        dynamic_dispatch: false,
    };
    sys_main_startup_initialize {
        displaced_len: 4,
        guard_sha256: "07f4993f36ec2892ba7daa618956f411293bf2fc693e4a3a2aebb9abd0c2ea04",
        continuation: Linear,
        dynamic_dispatch: false,
    };
    midi_in {
        displaced_len: 4,
        guard_sha256: "ba42a0ffa6ad2f1fed1a57c4268d425270224d416926807254d0b9b20f921efc",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    mixer_apply_channel_parameter {
        displaced_len: 4,
        guard_sha256: "48117e0eeae09544e6701eda1b9d74942930a0abe6425b20b2152b34eb815c9f",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    dsp_stream_callback {
        displaced_len: 4,
        guard_sha256: "90d0f4dca8b9cfa14674e9ea1f57cd8990bc17f089a15557bb19199796fb4535",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    dsp_effect_workspace_clear {
        displaced_len: 4,
        guard_sha256: "76ec330a358f97b9b7c3a577a461f97ec7d7ef19b6bb25ea9d988c82a13d7b25",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    mixer_set_efx_type {
        displaced_len: 4,
        guard_sha256: "7eae2449bb96f680058601cdadbe8dc619abd27106ed2ebb7f3bdbdd7986df0e",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    mixer_select_efx_callback {
        displaced_len: 4,
        guard_sha256: "737d21ab96d81780dcfdb407fe8c753b445433b5edeb879afa1e28f29e847bbf",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    rec_play_start_callback {
        displaced_len: 6,
        guard_sha256: "a8f15d4890025d07dcc510d06e44a592c054a41e28bb3ed1a68a79d1c915fde6",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    rec_play_stop_callback {
        displaced_len: 4,
        guard_sha256: "8171b1abec743fbec469239f56b25000f392712d4b58933e1cea2fff94726b2f",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    rec_start_callback {
        displaced_len: 6,
        guard_sha256: "f14800822781d0e786bb1629bd5b16bba41dfb32b77ff9e2a9e1c103689a737d",
        continuation: RelocatedBl,
        dynamic_dispatch: true,
    };
    rec_stop_callback {
        displaced_len: 4,
        guard_sha256: "8171b1abec743fbec469239f56b25000f392712d4b58933e1cea2fff94726b2f",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    panel_handle_main_button_event {
        displaced_len: 4,
        guard_sha256: "f1231a43486723ba9f8f321e7c7721e9cf5fab829d4319788572fe84e4bb067c",
        continuation: Linear,
        dynamic_dispatch: true,
    };
    midi_uart_rcv_msg_task {
        displaced_len: 4,
        guard_sha256: "a30a13e40c913a9c20c780f50b9e3c685038c53dca0f06a916ca7dad691d1a70",
        continuation: Linear,
        dynamic_dispatch: false,
    }
}
