# Notification synchronization and knob popup lifecycle

[Documentation index](../../README.md)

## Implemented design

Preserve the original notification manager and serialize complete operations
with a shared lock. Retain its queue, synchronous calls, timer, drawing routines
and busy-overlay behavior. Include the patch's pointer lookup, text/layout
changes and cleanup in the same synchronization boundary.

Mixer application remains outside that boundary and executes first. The hook
copies a pending value and the existing GUI callback performs presentation.
The hook never waits for the notification lock or compositor. Allocation failure
or suppressed presentation must never cancel, postpone or retry the mixer change.

The implemented extension retains the existing counting semaphore and uses a
36-byte heap sidecar for same-task nesting and copied presentation data. Stock
operations execute their original routines through locked veneers. This is
application-level nesting, not a FreeRTOS recursive mutex; it adds no priority
inheritance. Kernel calls receive the original untagged semaphore handle.
See the [patch documentation](../../../patches/knob-dialog/README.md) for executable
commands and the exact hook manifest.

Restore the patch's bindings before a transition renders the next stock
notification. Cleanup after the original tick returns is too late if pop has
already drawn a successor with the patch's resources. Keep allocation/free
inside the protected lifetime; a permanent allocation is not required to solve
that race once every relevant reader and writer participates.

A complete owner-task migration would change scheduling, request completion and initialization. It was not needed for the selected extension, which keeps the original manager and presentation callback.

Neither design establishes that the stock firmware has an observed concurrency
bug. The patch introduces object lifetime and resource changes that need an
explicit ownership rule.

## Manager transactions

The existing maximum-count-one counting semaphore remains the exclusion
primitive. A 36-byte heap sidecar stores the real handle, owner task, nesting
count and one copied pending value. The handle slot tags the sidecar pointer;
wrappers decode it before calling the original FreeRTOS take/give functions.
The compositor worker does not acquire this manager lock.

Same-task nesting is necessary because stock show, tick and busy operations
can call pop synchronously. Nested operations increment the sidecar depth;
only the outer transaction takes and releases the real semaphore. This is
not a FreeRTOS recursive mutex and does not add priority inheritance.

Knob handling first applies the original mixer change exactly once. It then
publishes the latest optional title/value event under a short critical section,
without taking the manager semaphore. Existing DisplayCycle2 callback slot 4
consumes the event under the lock and checks the recorder window and stock
notification priority. Presentation can be suppressed without losing the mixer
change. The callback remains registered while idle to service the first event.

Stock writers and resource readers use guarded entry points. Active/current
queries keep their original nonblocking reads because they expose flags and
bounded IDs, not pointers into the popup allocation. A window transition cancels
pending presentation and detaches before invoking arbitrary window callbacks;
it does not hold the manager lock across those callbacks.

## Resource lifetime

The popup uses one 1,788-byte allocation from the original firmware heap and
reuses it while refreshing. Stock transitions and expiry restore text resources,
bitmap bindings and widget styles before stock pop can draw a successor. Cleanup
clears the temporary text rectangle before restoring the original layout, then
frees the allocation. A stock Done remains distinct from the custom popup even
though both use notification ID 15.

An idle recorder callback calls busy-hide with busy flag 0. Stock busy-hide is a
no-op in that state. The current wrapper detaches only when the flag is 1 under
the manager lock, preserving the popup during idle polling. See the
[trigger mapping](popup-triggers.md).

If the sidecar allocation fails, the original semaphore remains usable and knob
presentation is disabled. If a popup allocation fails, the mixer still changes.
The sidecar lives until reset; popup allocations are released on detach.

## Validation and limits

Forced QEMU checks cover stock Done replacement, expiry-boundary encoder input,
full-queue nesting, busy pause/resume, popup allocation failure, and idle recorder
polling. Under a held manager semaphore, six original mixer calls were observed
while presentation remained blocked. SD update/restart checks preserve storage
regions and exercise all ten knob modes. The corrected package
was also installed and its popup behavior confirmed on an L6max.

- GDB stops perturb timing; forced interleavings are behavioral checks rather
  than physical latency measurements.
- Computed callers, real heap exhaustion, sidecar initialization failure,
  priority inversion and physical cache/DMA behavior need further coverage.
- Successful installation of this package does not establish the missing
  bootloader's complete validation or power-loss policy.
- No stock firmware race is claimed: the patch introduces resource lifetimes
  that require this additional ownership discipline.

## Alternatives not implemented

- A permanent allocation removes expiry-time free but does not prevent mixed
  resource bindings or concurrent queue mutations.
- A FreeRTOS recursive mutex would require mapped recursive APIs and constructor;
  replacing the handle without changing ordinary take/give calls is insufficient.
- A dedicated notification owner task would need synchronous request/reply
  semantics, audited caller lock ordering, bounded queue handling, and ordered
  tick forwarding. It adds scheduling changes beyond this feature's needs.
- A dedicated ROM frame could avoid the RAM bitmap copy, but it is not required
  by the current lock and lifetime design.
