# Testing workflow

Run commands from the tooling repository root. Start with [current status](current-status.md).
This guide covers executable checks for the emulator and its tools.

## One entry point

```sh
cargo run --offline --locked -p l6test -- list
cp tests/smoke/fixtures.example.toml smoke.local.toml
# Edit smoke.local.toml: pin QEMU by SHA-256 and supply local firmware/state/card paths.
cargo run --offline --locked -p l6test -- check --config smoke.local.toml
cargo run --offline --locked -p l6test -- run --suite quick --backend both
```

`check` validates the QEMU hash and fixture identities, checks shared memory,
and creates disposable fixture copies without launching firmware. `run` builds
selected binaries once, freezes copies of those executables and the scenario
files for the run, starts each case with fresh state/card copies, and
supervises every process group. A supplied output directory must not exist.
Paths in the fixture file are relative to that file. Both models are detected
from firmware and checked against the fixture's declared model.

To build a patched fixture, set `firmware_package` to a supported stock package
and add, for example, `patches = ["midi-sync"]`. The runner applies the same
guarded patch implementation used by the browser patcher and unpacks the
result inside each case's artifact directory. See the commented `knob-patched`
example in `fixtures.example.toml`. A saved MAIN NOR overrides the supplied
firmware, so a `state_template` containing MAIN must already match the generated
image; otherwise the runner rejects it. Inputs are never modified.

To assemble a saved state, set `state_recipe` to a TOML scenario containing
semantic setup actions and assertions. The runner starts a separate headless
guest with a fresh persistent state, runs the recipe, stops that guest and then
starts the requested case with the resulting state and copied card. Recipe and
included fragments are snapshotted in `state-setup/recipe`; setup logs and its
scenario result are in `state-setup/`. A recipe must support the selected model
and headless backend and cannot be combined with `state_template` or a volatile
case. `check` validates and snapshots fixtures but does not execute recipes.
The sample `state-l6max-setup.toml` describes first-boot date/settings setup;
it does not prepare recorder media or pad assignments.

To start from a configured device without driving its UI, set `state_spec` to
an [`l6nor` state spec](tools/l6nor.md#state-specs). The runner seeds missing
MAIN NOR from the fixture firmware and commits the declared settings with the
firmware's bank protocol. It also writes the declared panel RTC calendar and,
for L6max, a pre-programmed panel, all into the case's disposable state. No
extra guest runs. The spec hash and the applied report are recorded in
`inputs.json`. [`l6max-setup-complete.toml`](../../tests/smoke/states/l6max-setup-complete.toml)
reproduces the setup recipe's MAIN NOR byte for byte and boots directly to the
recorder; the `state-spec` case checks this. A spec can edit a copied
`state_template` and cannot be combined with `state_recipe`. Because a spec is
rebuilt for every case, volatile cases keep it: they run on the fresh
spec-built state instead of no state, and skip their setup input.

The driver bypasses GUI demo-media and installer defaults. Headless and GUI
runs receive the same explicit firmware, timing, card, state and no-audio
backend. Existing device state and fixture templates are never run in place.
The config may contain proprietary local paths and stays ignored. Add `fresh`
fixtures for both models to run the USB no-card probe; L6max persistence also
uses `fresh`. These cases reproduce explicit first-boot sequences. Prepared
mixer/card fixtures are used by the panel, SD and USB-storage cases. Use an
empty prepared card: after reinsertion, a card holding a recorder project shows
a new project named from the guest clock, which the L6max `storage` display
check rejects.
The factory-protocol scenario requires a `factory-protocol/l6max` firmware
fixture, but creates its own disposable FAT jig card and state under the case
artifact directory. The generated card and state hashes are recorded in
`inputs.json`.

## Fixture and catalog TOML

`smoke.local.toml` is private machine configuration; the example is
[`tests/smoke/fixtures.example.toml`](../../tests/smoke/fixtures.example.toml).
It has these top-level blocks:

| Block | Fields | Purpose |
| --- | --- | --- |
| top level | `version = 1`, `qemu`, `qemu_sha256` | Pin the exact QEMU executable. `qemu` is resolved relative to this TOML file; the SHA-256 must match before any run. |
| `[[fixture]]` | `name`, `model`, exactly one of `firmware_dir` / `firmware_package`; optional `patches`, `update_package`, `state_template`, `state_recipe`, `state_spec`, `sd_image`, `sound_pad`, `audio_device`, `gdb`, `timing`, `update_bootloader` | Define local firmware and starting state for a named model fixture. Names may repeat for different models, but `(name, model)` must be unique. |
| `[legacy."CASE-ID"]` | optional `fixture`, `executable`, `executable_sha256`, `arguments`, `environment` | Configure a legacy executable required by a catalog case. An executable requires a matching hash. `arguments` and environment values support `{firmware_dir}`, `{logs}`, `{qemu}`, and `{package}` substitutions. |
| `[[expected_failure]]` | `case`, `model`, `step`, `contains`, `reason` | Match a known failure only at the exact scenario step and when its error contains the specified text. A matching failure is XFAIL; an unexpected pass is XPASS. |

Fixture details:

- `model` is `l6` or `l6max`. `timing` is `paced` (default), `adaptive`,
  `host-clock`, `host-clock-multi`, or `split`.
- `patches` applies named guarded patches to a stock `firmware_package` before
  the driver creates its fixture copy. It cannot be combined with
  `firmware_dir`.
- Choose at most one of `state_template` and `state_recipe`. A template is
  copied into isolated case state. A recipe is a headless scenario that builds
  state on a separate disposable guest; see the complete
  [scenario/state-recipe schema](scenario-format.md). `state_spec` declares
  NOR settings, RTC and panel state without a setup guest, alone or on top of
  a template; it must name the fixture's model.
- If `state_template` is omitted, the driver creates fresh isolated state.
  `sd_image` is copied for each case. `audio_device` accepts QEMU audio-device
  syntax and `{logs}` expands to the per-case log directory. `gdb` enables the
  fixture's debugger socket. `update_bootloader` enables that update path.
- Add one or more `[[fixture.sound_pad]]` blocks to prepare assignments on the
  disposable `sd_image` copy. Each block takes `pad` (1–4), `file` (a path
  relative to the card root), optional `mode` (`one-shot`, `loop`, or `hold`;
  default `one-shot`), and optional `level` (0–59; default 49). The WAV must
  already exist on the card. The driver creates or updates the model's saved
  pad settings; it does not send USB commands or press controls to configure
  the device. The resulting card hash is included in `inputs.json`.

  ```toml
  [[fixture.sound_pad]]
  pad = 1
  file = "SOUND_PAD/PAD1/TONE220.WAV"
  mode = "one-shot"
  level = 59
  ```
- `update_package` is supplied to firmware-update scenarios as the package to
  install after booting the fixture firmware.
- All fixture paths resolve relative to `smoke.local.toml`; they must exist,
  match the declared model, and remain local. Firmware and device images are
  not checked in.

`tests/smoke/catalog.toml` is the checked-in registration of runnable cases.
Its top-level `version` is `1`; every `[[case]]` has:

| Field | Required | Purpose |
| --- | --- | --- |
| `id` | yes | Stable case identifier. |
| `suites` | yes | Suite names that include this case; an empty array means select it only with `--case`. |
| `models` | yes | Supported model list (`l6`, `l6max`). |
| `scenario` | one of `scenario`, `package`, or configured legacy executable | Path under `tests/smoke` to a scenario TOML file. |
| `package` | optional | Named GUI package operation for built-in app/package cases. |
| `arguments`, `environment` | optional | Arguments and environment for a package or legacy executable. |
| `needs_configuration` | optional, default `false` | Keep the case BLOCKED until its local executable/configuration prerequisite is supplied. |
| `gui` | optional, default `false` | Mark a GUI-only catalog case. |
| `volatile` | optional, default `false` | Discard persistent-state templates for this case and run without persistent state, unless the fixture has a `state_spec`, which is rebuilt for the case; incompatible with `state_recipe`. |
| `no_sd` | optional, default `false` | Do not attach the fixture's SD image. |
| `fixture` | optional | Use a fixture name other than the default `prepared`. |
| `blocked` | optional | Human-readable reason to report this case as BLOCKED. |
| `wall_timeout_s` | optional, default `900` | Per-case host wall limit. |

The local configuration, catalog entries, and scenario headers reject unknown
fields. Scenario action blocks are tagged by the required `action` value and
their action-specific fields are strict; use the documented common step fields
and action schema. A case's `scenario` path is relative to `tests/smoke`; scenario
`include` paths are relative to the containing scenario and cannot escape its directory.
`l6test`'s focused [command reference](tools/l6test.md) summarizes selection
flags and links to the full scenario schema.

Use the required execution permissions before starting suites: QEMU needs
POSIX shared memory and inherited sockets. Git operations on a worktree need
access to its parent repository's metadata. Repository instructions cannot
grant sandbox permissions; the calling environment must provide them.

## Suites and isolation

| Selection | Coverage |
| --- | --- |
| `--suite quick` | Shared boot, channel-1 scroll, channel-2 vertical drag and horizontal reversal, then mute feedback in one boot per model/backend |
| `--case boot` | Shared sweep/readiness gate only |
| `--case encoder` | Isolated channel-1 encoder output and exact firmware report count |
| `--case mute` | Isolated channel-1 mute feedback |
| `--suite regression` | Composed shared panel scenario, shared audio callbacks and SAI/eDMA stream checks, plus legacy full input |
| `--suite panel` | Full panel sequence through the selected adapters, one boot each |
| `--suite devices` | Shared audio callbacks and SAI/eDMA stream checks, SD presence/removal, USB MIDI/descriptors and L6max USB storage |
| `--suite lifecycle` | Shared power/restart and fresh L6max persistence |
| `--suite full` | Registered scenarios and remaining specialized legacy cases; missing prerequisites produce BLOCKED |

Cases run concurrently, up to `--jobs N` at once (by default a quarter of
the host's cores, between 1 and 4). Scenario waits use guest time, so
concurrency lengthens wall time. Keep N well below the core count; an
oversubscribed host can exceed wall limits. GUI cases, native host-MIDI
scenarios and external executables run alone. `--jobs 1` runs cases
sequentially.

Add `--model l6` or `--model l6max` to focus a run. `--backend both` runs
identical shared scenarios through headless and GPUI adapters on separate
copies of the same fixture. Legacy cases execute their declared backend only;
a low-level diagnostic pass does not qualify GUI parity.

`--no-build` explicitly reuses installed debug executables. Their hashes are
still recorded. It is useful for repeated diagnostic runs and binary comparisons,
but does not attest that an executable was built from the current sources.
The driver currently pins QEMU by binary hash and explicitly records its build
source as unattested. A source checkout hash alone never identifies that binary.
It never auto-selects, rebuilds or falls back to a different QEMU.

Boot readiness requires the model's six or eight rings to reach
all-lit and settle for one guest second; L6 also needs a lit editing-mode
lamp, and L6max needs display pixels. On template and fresh state, L6max also
waits for a 12 guest-second floor: its first-boot Date/Time editor appears at
about 9 s, after a card popup, with no lamp change. Spec-built state boots
straight to its home screen, so it skips the floor; L6max then also requires one
guest second without a new display frame. Both sweeps end within 5 guest
seconds. Shared control scenarios then wait until every visible ring holds a
restored value (on L6, also a lit mode lamp), select LEVEL unless it is already
lit, verify its lamp, and wait for stable panel outputs before capturing
per-control baselines. A static OLED
frame is valid and must not be treated as a stopped guest clock.

Prepared fixtures must match the scenario's explicit setup steps. With a
saved-state template, the L6max sequences select Cancel with Down and then
Confirm on the startup SD prompt, or Confirm the startup prompts twice; they
are not general Date/Time or arbitrary-menu navigation. These steps are marked
`setup_input`. A fixture with a `state_spec` boots straight to the recorder,
so the driver skips them and records `execution_spec_state` in `inputs.json`.
Keep fresh first-boot fixtures and prepared fixtures separately named.

## Results and evidence

The printed artifact directory contains:

- `report.json` and, for completed suites, `summary.md`: matrix and outcome.
- `manifest.json`: source revision, dirty diff/untracked hashes, board/patch
  hashes, driver binary and pinned QEMU identity.
- Per-case `inputs.json` and `invocation.json`: selected fixture content hashes,
  execution options, executable hash, exact arguments and declared environment.
- `scenario.json`: shared assertions, failed step, last state and bounded events.
- `stdout.log`, `stderr.log`, QEMU trace, LCD PNG and ASCII preview; named checkpoints also retain PNG/ASCII.
  SD tracing selects `sdhci.trace`; other runs use `engine.trace`. Restart logs
  live under the case's `restart/` directory.

Mapping assertions wait for quiet visible rings and UART reports before advancing
(one guest second by default, three in the full-ring fragment); this prevents intermediate values being mistaken for
completed actions. L6 compares only its six physical rings.

Default artifact directories are short paths under `/tmp` so Unix socket paths
fit platform limits. Retain useful runs outside temporary storage before cleanup.
`--output /short/new/path` selects a retained location. Completed cases are written
incrementally. Exit 0 means PASS or explicitly matched XFAIL, 1 means FAIL or
XPASS, and 2 means BLOCKED. Read the outcome rather than inferring a clean pass
from exit 0. Keep a baseline with XFAIL labeled as incomplete.

Expected failures are optional fixture-config entries matching an exact
case/model/step and error substring, with a reason. A different failure remains
FAIL; an unexpected pass becomes XPASS and requires review. No current device
failure is hidden by a checked-in expected-failure rule.

The process supervisor bounds wall time, stops descendant processes on failure,
interruption or completion, and leaves the system mouse untouched. Scenario
waits and button timing use guest time, with independent wall-clock ceilings.

## Scenario format and extension

See [the version-1 format](scenario-format.md). New behavior belongs in a shared
scenario or a typed operation in `smoke-core`, not another one-off binary.
`tests/smoke/catalog.toml` registers cases, model scope, suites, legacy prerequisites
and time limits. WASM patch parity runs as a typed host-side driver operation.
The focused panel cases compose reusable fragments rather
than copying setup or asserting results separately.

Packaged application tests use the same supervisor and report format. Configure
`[legacy."gui-bundle"]` and `[legacy."gui-menu"]` in `smoke.local.toml` with the
packaged GUI executable and its SHA-256, each test's isolated `L6_APP_DATA`, and
the bundled QEMU SHA-256. The example fixture file contains the environment
variables and firmware paths for both cases. Run them on macOS with:

```sh
cargo run --offline --locked -p l6test -- run --case gui-bundle --model l6max --backend gui
cargo run --offline --locked -p l6test -- run --case gui-menu --model l6max --backend gui
```

`gui-bundle` imports L6 then L6max firmware and checks isolated persisted state;
`gui-menu` checks both native picker actions while an L6max guest is running.

## Structured runtime logs

The driver, shared runner and GPUI scenario adapter
use `tracing`. Smoke execution writes one JSON event per line to stderr by
default. `list` and help retain ordinary stdout. Case child stderr is captured
in `stderr.log`; QEMU's device trace is a separate artifact.

```sh
RUST_LOG=warn,l6test=info,smoke_core=debug cargo run --offline --locked -p l6test -- run --suite quick
L6_LOG_FORMAT=text cargo run --offline --locked -p l6test -- run --case boot
```

`L6_LOG_FORMAT=json|text` chooses JSON or compact text. `RUST_LOG` accepts normal
`tracing_subscriber::EnvFilter` directives. Invalid settings fail before boot.
The driver records and forwards the resolved logging settings. JSON events carry
levels, targets, timestamps, case/model/backend span context and numeric step/
guest-time fields. Debug events contain detailed runner observations; the default
filter limits output to progress, step boundaries and results. Report and manifest
JSON files remain the stable test-result interface.

The shared QMP client emits command names, elapsed milliseconds and outcomes at
DEBUG level, failed commands at WARN, and asynchronous event names at DEBUG. It
does not log command arguments or memory/register contents. Add
`l6max_host::qmp=debug` to `RUST_LOG` when investigating QEMU management traffic.

The regular GUI defaults to compact text and honors the same settings. Remaining
legacy diagnostic binaries may still print their own historical text output.

## Unit and build checks

```sh
cargo fmt --all --check
cargo check --offline --locked --workspace --all-targets
cargo test --offline --locked -p smoke-core -p l6test -p l6max-host -p l6max-gui
```

These firmware-free checks verify parsing, sequencing, deadlines, snapshots,
transport, gesture handling and process cleanup. They do not establish guest
firmware behavior. See [CI](ci.md) for hosted checks.

The panel register check uses the built QEMU test accelerator and a synthetic
image; it needs neither firmware nor an SD card:

```sh
python3 emulator/qemu/tests/panel-qtest.py emulator/qemu/build-source/build/qemu-system-arm
```

It exercises both panel profiles, flash boundaries and existing 64 KiB saved
flash, plus GPIO and timer registers, pending interrupts and timer overflows.
It does not establish behavior on physical hardware. The smaller C tests in
`emulator/qemu/tests/panel-gpio.c` and `panel-timer.c` cover shared register
logic independently of QEMU.

## Input, display, and LEDs

Use the shared `panel`, `l6-input-sweep`, and `l6max-input-smoke` scenarios.

## Integration setup

Use the fixture configuration above and the scenario catalog for prerequisites.

## Firmware patch

Patch, factory, and update cases need explicit local configuration and matching
firmware. Use `knob-dialog`, `popup-concurrency`, `patched-update`,
`firmware-update`, and `wasm-patch` as appropriate.

## Power, storage, and USB

Use the `lifecycle` or `devices` suite and the relevant individual cases.

## Editor

Use the `editor-pad-assignments` case for the emulator's editor protocol path.
