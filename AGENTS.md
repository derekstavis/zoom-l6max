# Working in the tooling repository

This repository owns the simulator, package/patch tools and executable tests.
Keep changes on a feature branch until the user approves a merge.

Read `docs/development/current-status.md`, then `docs/development/testing.md`.
Use `cargo run --offline --locked -p l6test -- list` and the consolidated driver.
For focused runtime observations outside qualification, use
`docs/development/diagnostics.md` and the `l6dbg` tool documentation.
Local firmware/state paths belong in ignored `smoke.local.toml`; never commit
firmware, SD cards, saved flash, generated analysis artifacts or runtime logs.

Before suites, use execution permissions that allow POSIX shared memory and
inherited sockets. Shared Git worktrees need access to their parent Git metadata.
These requirements cannot be granted by this file; request the tool's required
execution permissions directly rather than repeatedly retrying a denied suite.

Both GUI and headless scenarios use `smoke-core`, `PanelFeed` and `PanelData`.
Keep assertions out of GUI adapters. Prefer guest-time readiness/output checks
with bounded wall deadlines. A static OLED is valid. L6 has six rings and no screen;
L6max has eight rings. Do not substitute one model's persistent state for the other.

Use the reference manual for peripheral behavior. Preserve compatibility
exceptions with explicit model scope and evidence. Scope firmware-specific
addresses and compatibility behavior to the corresponding revision.

Record exact binary and fixture identities before claiming a pass. A source
commit or QEMU version string does not identify a QEMU executable. The driver
pins external QEMU binaries and explicitly marks their source as unattested.

Read the existing working-tree status before editing. Preserve unrelated changes.
Never merge or squash history implicitly.
