// Hookable L6 MAIN 1.10 function entries and loader metadata.

define_function_hooks! {
    firmware_bindings;
    fw_mount {
        displaced_len: 4,
        guard_sha256: "cfa045938b57940b57930f97cd7cdf3e1a8c3b1ddb72c2609b95b97c1b1921ed",
        continuation: Linear,
        dynamic_dispatch: false,
    };
    l6_initialize_and_start_scheduler {
        displaced_len: 4,
        guard_sha256: "d616a7757d49d949705c5a0888c6c6406e7255dca579d24ab7eee1d0052ef245",
        continuation: Linear,
        dynamic_dispatch: false,
    };
    midi_in {
        displaced_len: 4,
        guard_sha256: "25ab4500d49822a9cbf8d068b2823a1f42f7f6b1ad4c6db6025e8b14eaad1b74",
        continuation: Linear,
        dynamic_dispatch: true,
    }
}
