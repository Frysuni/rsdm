# Audit remediation

Baseline: `1930481` (2.2.2). Finding numbers refer to the local `audit.md`.

Each finding must be checked against its callers before changing code. Confirmed
defects receive a regression check and a separate commit. Related documentation
claims are closed with the implementation that makes them accurate. A finding
that does not match current behavior needs evidence, rather than a speculative
change. Version bumps and pushes require separate maintainer authorization.

Run lifecycle tests in isolated fixtures or VMs; never restart the host session
to reproduce a defect. Preserve generation ownership, invocation validation,
session leases, and the lock protocol's fail-secure behavior.

Statuses: pending, fixed, not reproduced, policy exception.

| Finding | Work | Status |
| --- | --- | --- |
| 1 | One shutdown deadline across all teardown phases | pending |
| 2 | Bound application lease acquisition | pending |
| 3 | Release application leases before slow shutdown work | pending |
| 4 | Durable recovery record publication | pending |
| 5 | Symmetric record read/write size limits | fixed |
| 6 | Reap lockers that never confirm readiness | pending |
| 7 | Bound idle hooks | pending |
| 8 | Retain restoration state for removed niri outputs | pending |
| 9 | Move bounded niri IPC off the Wayland loop | pending |
| 10 | Cancellable, bounded PAM helper processes | pending |
| 11 | Remove complex post-PAM fork child work | pending |
| 12 | Pin emergency unlock targets with pidfds | pending |
| 13 | Validate lock state through one open descriptor | pending |
| 14 | Honor PAM-requested authentication delay | pending |
| 15 | Account throttling after PAM identity mapping | pending |
| 16 | Audit successful starts after launch confirmation | pending |
| 17 | Audit termination separately from cleanup failure | pending |
| 18 | Move control-bus maintenance off the actor | pending |
| 19 | Release transport mutex during async reconnect | pending |
| 20 | Scale reference restoration within bounded work | pending |
| 21 | Roll back definitely failed app registrations | pending |
| 22 | Roll back definitely failed start references | pending |
| 23 | Recover generation-owned quit/logout helpers | pending |
| 24 | Bound shutdown concurrency and polling | pending |
| 25 | Linear process snapshot/pidfd signaling | pending |
| 26 | One recovery deadline across all teardown phases | pending |
| 27 | Establish shutdown backstop before child startup | pending |
| 28 | Recover console log level after greeter crashes | pending |
| 29 | Establish sane terminal baseline after crashes | pending |
| 30 | Generic autovt ownership | pending |
| 31 | Synchronize generic service and configured VT | pending |
| 32 | Avoid disabled-DM service restart loops | pending |
| 33 | Reuse render buffers and avoid unnecessary frames | pending |
| 34 | Cache scaled wallpaper | pending |
| 35 | Recover output-power changes after locker crashes | pending |
| 36 | Harden remembered-state reads | pending |
| 37 | Harden privileged log file opens | pending |
| 38 | Avoid console font temporary-file collisions | pending |
| 39 | Reap closed generation lease files safely | pending |
| 40 | Bound coordinator request backlog | pending |
| 41 | Bound launch request payloads | pending |
| 42 | Clarify/update Arch release package versions | pending |
| 43 | Verify downloadable Arch package artifacts | pending |
| 44 | Narrow release token write permissions | pending |
| 45 | Pin third-party actions to immutable commits | pending |
| 46 | Pass only required reusable-workflow secrets | pending |
| 47 | Pin AUR SSH host identity | pending |
| 48 | Add Clippy to CI | policy exception |
| 49 | Choose one deterministic automatic keyring | pending |
| 50 | Follow Desktop Entry Exec parsing semantics | pending |
| 51 | Reject ambiguous fixed-session display names | pending |
| 52 | Use logind power capabilities for lock UI | pending |
| 53 | Validate ready timeout representability | pending |
| 54 | Document pidfd emergency unlock protection | pending |
| 55 | Match bounded recovery-record documentation | fixed with 5 |
| 56 | Match bounded-worker architecture claims | pending |
| 57 | Correct obsolete first-seat documentation | pending |

Finding 48 conflicts with the explicit policy in
[CONTRIBUTING.md](../.github/CONTRIBUTING.md). The policy remains in force;
concurrency fixes and regression tests do not require changing it.

Work order: recovery records and leases; shutdown deadlines and helpers; idle
and output lifecycle; authentication and process identity; actor responsiveness;
TTY and packaging; rendering, API bounds, release configuration, and remaining
compatibility/documentation findings. Dependencies may require changing this
order. Mark a row fixed only after its behavior has been checked.

## Completed checks

- 5, 55: recovery writes reject serialized TOML over the same 1 MiB read limit
  before creating temporary files. Tests round-trip the exact boundary, preserve
  previous state after an oversized update, and reject oversized session state.
