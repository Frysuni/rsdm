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
| 2 | Bound application lease acquisition | implemented; runtime checks pending |
| 3 | Release application leases before slow shutdown work | fixed |
| 4 | Durable recovery record publication | fixed |
| 5 | Symmetric record read/write size limits | fixed |
| 6 | Reap lockers that never confirm readiness | pending |
| 7 | Bound idle hooks | pending |
| 8 | Retain restoration state for removed niri outputs | pending |
| 9 | Move bounded niri IPC off the Wayland loop | pending |
| 10 | Cancellable, bounded PAM helper processes | pending |
| 11 | Remove complex post-PAM fork child work | pending |
| 12 | Pin emergency unlock targets with pidfds | implemented; runtime checks pending |
| 13 | Validate lock state through one open descriptor | implemented; runtime checks pending |
| 14 | Honor PAM-requested authentication delay | pending |
| 15 | Account throttling after PAM identity mapping | pending |
| 16 | Audit successful starts after launch confirmation | fixed |
| 17 | Audit termination separately from cleanup failure | pending |
| 18 | Move control-bus maintenance off the actor | implemented; runtime checks pending |
| 19 | Release transport mutex during async reconnect | implemented; runtime checks pending |
| 20 | Scale reference restoration within bounded work | pending |
| 21 | Roll back definitely failed app registrations | pending |
| 22 | Roll back definitely failed start references | pending |
| 23 | Recover generation-owned quit/logout helpers | pending |
| 24 | Bound shutdown concurrency and polling | pending |
| 25 | Linear process snapshot/pidfd signaling | implemented; runtime checks pending |
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
| 44 | Narrow release token write permissions | fixed |
| 45 | Pin third-party actions to immutable commits | pending |
| 46 | Pass only required reusable-workflow secrets | pending |
| 47 | Pin AUR SSH host identity | pending |
| 48 | Add Clippy to CI | policy exception |
| 49 | Choose one deterministic automatic keyring | pending |
| 50 | Follow Desktop Entry Exec parsing semantics | pending |
| 51 | Reject ambiguous fixed-session display names | pending |
| 52 | Use logind power capabilities for lock UI | pending |
| 53 | Validate ready timeout representability | pending |
| 54 | Document pidfd emergency unlock protection | implemented with 12; runtime checks pending |
| 55 | Match bounded recovery-record documentation | fixed with 5 |
| 56 | Match bounded-worker architecture claims | pending |
| 57 | Correct obsolete first-seat documentation | fixed |

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
- 4: synchronize the temporary record before rename and its directory afterward.
  New generation directories also synchronize their directory entries. Record
  tests cover replacement and cleanup after publication failure. File-system
  durability barriers do not preserve `/run` across reboot or power loss.
- 16: emit session-started only after the launcher returns a handle. Login tests
  verify its ordering and its absence when launch fails, while preserving PAM
  closure and shutdown-inhibitor lifetime on all existing error paths.
- 3: leases cover record mutations only; invocation queries, quit requests, and
  process waits run after releasing them. The 70 session-manager tests passed,
  including concurrent lease acquisition during an app wait, single quit
  ownership, and cancellation identity checks. The Nix lifecycle build was
  interrupted when battery restrictions resumed; its VM test did not run.

Battery restriction: do not build or run compilation-based test suites. Use
Rust's syntax-only parse (`rustc -Z unpretty=normal --edition=2024`, output
discarded) and `git diff --check` for changes made under this restriction.
Record unexecuted runtime checks explicitly for later verification.

- 2: application leases use nonblocking flock with a 250 ms retry budget for
  short record updates. Contention and successful short-wait regression tests
  were added and syntax-checked; execute them when build restrictions are lifted.
- 13: lock-state reads open once with NOFOLLOW/NONBLOCK/CLOEXEC, validate that
  descriptor's owner/type/mode, and cap input at 4 KiB. Regression tests cover
  pathname replacement, symlinks, FIFOs, shared permissions, ownership, and size;
  only syntax checks have run under the battery restriction.
- 12, 54: emergency unlock pins a pidfd, repeats UID/start-time/executable/argv
  validation, checks that the pinned process remains alive, and signals only
  through that handle. The existing session pidfd primitive is shared through
  `unix::process_handle`; session ownership and ESRCH behavior remain unchanged.
  Syntax checks passed; pidfd and isolated emergency-unlock tests remain pending.
- 25: snapshot app processes once, pin at most 32 pidfds per batch, revalidate
  InvocationID after pinning, and check each pinned process's systemd cgroup in
  procfs. This avoids repeated full process lists and unbounded descriptor use.
  Query-count and hierarchy/boundary regression tests were syntax-checked only.
- 57: the architecture now describes every advertised/hotplugged seat and the
  all-connected-seats-idle gate, matching `idle::seats` and the idle guide.
  Documentation diff checks passed; no runtime behavior changed.
- 19: reconnect snapshots ownership under the state mutex, restores without it,
  and publishes only if epoch/revision still match. Stale or failed candidates
  are closed. An asynchronous maintenance guard serializes reconnect and
  activation-environment writes while ordinary state access remains available.
  Private peer tests cover a suspended RefUnit call, concurrent release, stale
  publication rejection, and guard timeout/drop; only syntax checks have run.
- 18: control endpoint health/reconnect runs in one cancellable worker; the
  coordinator only polls its bounded result channel. Private peer tests suspend
  the health reply and check caller responsiveness and cancellation on closure.
  Only syntax checks have run. Finding 56 still needs the remaining synchronous
  manager reads in observation/readiness/finalize to leave the actor loop.
- 44: release workflow defaults to contents:read. Only GitHub release publishing
  and stable-channel metadata commits request contents:write; build and reusable
  AUR jobs retain read access. YAML parsing and permission checks passed locally;
  no workflow was dispatched.
