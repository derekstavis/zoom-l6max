// Hookable L6max MAIN 1.10 function entries and loader metadata.

define_function_hooks! {
    firmware_bindings;
    fw_mount {
        displaced_len: 4,
        guard_sha256: "cfa045938b57940b57930f97cd7cdf3e1a8c3b1ddb72c2609b95b97c1b1921ed",
        continuation: Linear,
        dynamic_dispatch: false,
    };
    main_startup_initialize {
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
    midi_uart_rcv_msg_task {
        displaced_len: 4,
        guard_sha256: "a30a13e40c913a9c20c780f50b9e3c685038c53dca0f06a916ca7dad691d1a70",
        continuation: Linear,
        dynamic_dispatch: false,
    }
}
