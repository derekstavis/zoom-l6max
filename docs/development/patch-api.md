# Rust patch payload API

Patch payloads are small `no_std` Rust programs compiled directly by
[`l6patch`](tools/l6patch.md), not Cargo crates. They include the shared
[`patchrt.rs`](../../patches/api/patchrt.rs), which provides
firmware bindings and hook attributes without adding a runtime library:

```rust
#[path = "../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
```

Use the model-specific declarations in [`patches/api`](../../patches/api/README.md)
when available. Functions are grouped under each model by subsystem, and its
`hooks.rs` is the shared hookable-site catalog. Add patch-local firmware
bindings only for functions or variables that are genuinely patch-specific.

## API at a glance

| Need | API | What it does |
| --- | --- | --- |
| Declare callable firmware functions | `firmware_functions!` | Creates typed unsafe wrappers at revision-specific addresses. |
| Describe firmware functions that can be patched | `define_function_hooks!` | References `firmware_functions!` declarations and adds guard, displaced-length, continuation, and dynamic-dispatch metadata. |
| Read or write firmware globals | `firmware_variables!` with `.read()` and `.write()` | Defines typed volatile descriptors; only `mut` declarations permit writes. |
| Hook every function entry | `#[function_entry_hook(site)]` | Adds a handler to an entry hook chain; choose `hook_continue` or `hook_done` on each path. |
| Inject at a function instruction offset | `#[function_offset_hook(function, at = OFFSET)]` | Generates a register-preserving static veneer; `hook_continue()` replays displaced instructions and `hook_done()` skips them. |
| Add behavior around a call to a function | `#[function_call_pre_hook(function, at = ADDRESS)]`, `#[function_call_post_hook(...)]` | Adds typed handler calls selected by the caller return address; the original `BL` stays in place. |
| Call a generated stock continuation | `call_original!(function, args...)` | Calls the guarded stock path without requiring a repeated ABI signature. |

All addresses and guards are specific to a firmware model and revision. The
builder validates hook metadata against the selected firmware before writing a
patched image.

## Firmware declarations

`firmware_functions!` binds a firmware function's C ABI signature and address.
The wrapper sets the Thumb bit. Calls are unsafe because the signature,
firmware revision, and task context must all be valid. The comma before `at`
is part of the syntax when a return type is present:

```rust
firmware_functions! {
    pub unsafe fn mixer_get_level(channel: u32) -> u32, at 0x8000_b0b0;
    pub unsafe fn midi_send_cc(parameter: u32, value: u32) at 0x8003_f9b0;
}

let level = unsafe { mixer_get_level(channel) };
```

The macro also exposes a type-level `Abi` and `ADDRESS` in the function's
namespace. Hook declarations use those so function names, signatures, and
addresses are not duplicated.

`firmware_variables!` creates descriptors for volatile globals. A declaration
without `mut` is read-only; `mut` permits writes. It does not create a Rust
mutable reference or synchronize with firmware tasks:

```rust
firmware_variables! {
    pub message_length: u16, at 0x805e_5d4a;
    pub mut status_word: u32, at 0x805e_5d50;
}

let length = unsafe { message_length.read() };
unsafe { status_word.write(1) };
```

Both operations are unsafe because the address, alignment, access permission,
and concurrent firmware activity must be valid.

## Shared hook catalog

The model-specific `hooks.rs` uses `define_function_hooks!` to describe
functions that the loader or patch builder can redirect. Addresses and ABIs
come from the model-specific `firmware_functions!` declarations; the catalog
adds guarded bytes and continuation behavior:

```rust
define_function_hooks! {
    firmware_bindings;
    midi_in {
        displaced_len: 4,
        guard_sha256: "...64 hex characters...",
        continuation: Linear,
        dynamic_dispatch: true,
    }
}
```

The guard covers the bytes replaced by a function-entry redirect. `Linear`
replays those instructions and resumes after them; special control-flow
instructions need an explicit continuation strategy. `dynamic_dispatch`
marks sites supported by the resident Patch loader. Patch declarations that name
a function absent from this catalog are rejected by the builder.

### Design decisions

- `define_function_hooks!` refers to typed functions declared by
  `firmware_functions!`. The function declaration is the source of truth for
  the symbol, address, and ABI; the hook catalog adds only metadata needed to
  safely redirect it. Builders must reject a catalog entry without a matching
  typed declaration.
- `function_offset_hook` is static-only. Its displaced instructions and live
  register context are specific to the exact firmware image, so it is not a
  dynamic SD registration type.
- In an entry hook, `hook_done` ends the chain and returns the supplied result
  (or `()` for a void function). In an offset hook, `hook_done()` skips the
  displaced instructions and resumes after them. Rust `return` remains an
  ordinary early return from the handler; it does not select either hook
  action.

Hook attributes and the shared catalog are the metadata source for static
patches, SD modules, and browser templates. Builders reject unknown sites,
invalid firmware call targets, and call handlers whose argument types do not
match the declared callee.

## Function-entry hooks

An entry hook has the same arguments and return type as the firmware function
it hooks. The attribute supplies the C ABI and emits registration metadata:

```rust
#[function_entry_hook(midi_in)]
pub unsafe fn midi_in_hook(message: *const u8) {
    if unsafe { is_sync_message(message) } {
        unsafe { broadcast_state() };
        hook_done()
    } else {
        hook_continue(message)
    }
}
```

Every path must finish with one of these actions:

- `hook_continue(args...)` calls the next hook in the chain, or the stock
  continuation after the last hook.
- `hook_done()` for a void function, or `hook_done(value)` for a function with
  a return value, ends the chain and returns without running later hooks or
  the stock body. It is not Rust's early `return`.

A static patch redirects the cataloged entry and generates its stock
continuation. The Patch loader can dispatch only catalog sites marked for dynamic
use. Entry attributes alone do not make arbitrary firmware functions
loadable.

## Function-offset hooks

An offset hook runs inside the original function at the declared byte offset
from its entry. Its helper takes no arguments: the generated veneer saves live
registers around the helper, then resumes the stock function. `hook_continue()`
replays the displaced instructions before resuming. `hook_done()` skips those
displaced instructions and resumes after them; use it only when the hook
replaces their behavior.
Offset hooks are static-only because their correctness depends on the exact
instruction sequence and live register state:

```rust
#[function_offset_hook(
    midi_uart_rcv_msg_task,
    at = 0x21e,
    displaced_len = 4,
    guard_sha256 = "...64 hex characters..."
)]
pub unsafe fn din_mixer_forward() {
    unsafe { forward_complete_message() };
    hook_continue()
}
```

The attribute includes a guard over the displaced instructions, since the
same function can have different instruction bytes at an insertion offset.
For an offset hook, `hook_done()` skips the displaced instructions and resumes
at the next instruction. It does not return from the containing function.

## Function-call hooks

A call hook names the callee and the firmware address of a `BL` instruction.
The builder verifies that the instruction calls the declared function. The
call instruction itself stays unchanged. A generated entry dispatcher checks
the caller return address and invokes matching handlers around the normal
callee path.

Pre and post handlers take the same typed arguments as the callee and return
`()`. A post handler does not receive or change the callee's return value. The
dispatcher preserves argument registers and stack arguments when it calls a
handler, and preserves the result registers across post handlers. Condition
flags are caller-clobbered by the C ABI. For example, the knob dialog reacts
after the stock mixer parameter update:

```rust
#[function_call_post_hook(mixer_apply_channel_parameter, at = 0x8003_6c74)]
pub unsafe fn knob_changed(
    channel: u32,
    mode: u32,
    absolute: u32,
    delta: i32,
    value: u32,
) {
    unsafe { refresh_dialog(channel, mode, absolute, delta, value) };
}
```

Call-site hooks add behavior; they do not replace or skip the callee. Multiple
handlers may target a function or call site. The caller's `BL` remains
unchanged. A dispatcher redirects the callee's entry and compares the saved
caller return address with each declared `BL` site before invoking matching
handlers. Dynamic call hooks require the callee to be marked `dynamic_dispatch`
in the shared catalog. Offset hooks remain static-only.

For dynamic modules, files are loaded in numeric filename-prefix order. Entry
and pre-hook handlers run in that order. Post-hook handlers run while the
wrapper chain unwinds, so their callback order is the reverse of the filename
order. This filename ordering describes SD modules; static patch composition
is determined by the selected payloads and the static builder, not SD filenames.

### Build-time patch dependencies

Patches can list other patch names in `requires` in
`patches/<patch>/package.toml`. The SD builder resolves the dependency closure,
checks for missing patches and cycles, then compiles the selected patches and
their dependencies into one loadable module. The source modules share one
patch runtime, firmware catalog, and allocation. Hook handlers for the same
function keep separate continuation slots and follow dependency order, then
the user's requested patch order.

The same package record is the source for patch names, versions, model and
installation support, dependency resolution, and the web and emulator catalogs:

```toml
[package]
name = "dual-efx"
version = "1.0.0"
kind = "patch"
order = 50
title = "Dual EFX"
summary = "Run two effects at once."
description = "Runs both send effects with per-channel routing and editable parameters."
models = ["L6max"]
firmware_models = ["L6max"]
sd_card_models = ["L6max"]
wasm_models = []
website = false
website_order = 0
website_sd_models = []
emulator_gui = false
emulator_order = 0
emulator_sd_models = []
readme = "README.md"
requires = ["knob-dialog"]
```

`firmware-patch-core` validates and enumerates these files through
`patch_catalog()`. Its build script generates the native package catalog and
the patch-name arrays from the same metadata. The web build exports that API
as JSON instead of maintaining a second patch list.

Composition is a build-time operation. It creates one BIN file named from the
requested patch names and versions followed by dependency-only patch names and
versions, such as `dual-efx-1.0.0-knob-dialog-1.0.0.bin`. A dependency shared
by multiple requested patches is included once in that combined file. Separate
builds each contain their own dependency copy. Removing the file removes all
included features together. No runtime patch registry, initializer, or
patch-to-patch function table is required.

## Calling an original continuation

Within an entry hook, use `hook_continue(...)` for that hook's own chain. Use
`call_original!` when code outside that hook needs the stock continuation of a
different function. The function must have a typed `firmware_functions!`
declaration:

```rust
unsafe { call_original!(notification_pop, 2) };
```

This is not a call to the patched firmware entry, which could re-enter the
hook. It calls the generated continuation and does not require writing
`extern "C"` or repeating its signature.

## Build and verification

Hook registrations and the shared catalog are emitted into the payload ELF's
`.patch_hooks` metadata section. Builders read that section to validate and
create required continuations and trampolines. Browser patch templates are
prebuilt from the same metadata. Hook guards and registrations are sourced
from the shared catalog and hook attributes; there are no `hooks.tsv` files.

Build each affected profile with `l6patch` and run its focused emulator
scenario. A successful Rust compile checks syntax and symbol generation, but
does not prove firmware signatures, displaced-instruction behavior, hook
ordering, or runtime safety.
