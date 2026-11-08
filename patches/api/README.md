# Shared patch API

Payloads include [`patchrt.rs`](patchrt.rs) because `l6patch` compiles each
payload directly with `rustc` rather than building a Cargo dependency graph.
Keep this runtime `no_std` and dependency-free.

For macro selection, syntax, hook flow, and examples, see the developer
[Rust patch payload API guide](../../docs/development/patch-api.md). This page
documents the shared implementation and address modules.

Bindings are grouped by model and subsystem:

```text
api/
  patchrt.rs
  l6/
    fs.rs
    hooks.rs
    memory.rs
    midi.rs
    mixer.rs
    rtos.rs
  l6max/
    fs.rs
    gui.rs
    hooks.rs
    memory.rs
    midi.rs
    mixer.rs
    rtos.rs
```

Each model's `mod.rs` includes its subsystem declarations into one module, so
existing function and variable names remain available to payloads. Add a
binding there when another patch can use the same firmware API; keep feature
state and private helpers in the patch itself. Addresses are revision-specific
and must be checked against the supported image before reuse on another
firmware version.

The `hooks.rs` files define hookable function entries and their guard,
continuation, and dynamic-dispatch metadata. Both the static builder and SD
loader use these model-specific catalogs. Loader implementation remains in
`patch-loader`; the hook catalog is shared API metadata.

The shared module also provides the default `no_std` panic handler for each
payload. It halts in a spin loop and does not call back into firmware, since a
panic may occur while firmware locks or interrupts are in an unsafe state.
This handles Rust panics only; Cortex-M faults use the firmware's exception
vectors.

`firmware_functions!` declares typed, unsafe calls. It adds the Thumb bit and
converts the address to the declared C ABI signature in one place:

```rust
firmware_functions! {
    pub unsafe fn mixer_get_level(channel: u32) -> u32, at 0x8000_b0b0;
    pub unsafe fn midi_send_cc(parameter: u32, value: u32) at 0x8003_f9b0;
}
```

The comma before `at` is required after a return type by the `macro_rules!`
grammar. Calls remain unsafe because the address, signature, firmware revision,
and task context must all be valid.

`firmware_variables!` creates typed volatile variable descriptors. Read-only
bindings use the default form; `mut` marks a binding writable:

```rust
firmware_variables! {
    pub message_length: u16, at 0x805e_5d4a;
    pub mut status_word: u32, at 0x805e_5d50;
}

let length = unsafe { variable_read!(message_length) };
unsafe { variable_write!(status_word, 1) };
```

The descriptors expose their address for APIs that need a pointer rather than a
scalar read. Reads and writes remain unsafe and use volatile operations. A
`mut` binding means firmware memory may be written; it does not create a Rust
mutable reference or guarantee synchronization with firmware tasks.

The L6max GUI bindings include the widget/background descriptor tables, the
localized text pointer and capacity roots, the DisplayCycle2 callback table,
fixed table counts, and display dimensions. The descriptor arrays and callback
table are stored directly at their binding addresses; the localized text
bindings instead hold root pointers, so use `variable_read!` to obtain their
tables. These common bindings let GUI patches share the reviewed address
definitions without moving feature-specific descriptor layouts or state into
the common module.

The function-hook attributes are re-exported from `patchrt.rs` and
implemented in the `patch-hook-macros` proc-macro crate. `define_function_hooks!`
uses model-specific `firmware_functions!` declarations for entry addresses and
types. Entry, offset, and call-site hook behavior is documented in the
[patch API guide](../../docs/development/patch-api.md).
