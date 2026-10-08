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
| 1 | One shutdown deadline across all teardown phases | fixed |
| 2 | Bound application lease acquisition | fixed |
| 3 | Release application leases before slow shutdown work | fixed |
| 4 | Durable recovery record publication | fixed |
| 5 | Symmetric record read/write size limits | fixed |
| 6 | Reap lockers that never confirm readiness | fixed |
| 7 | Bound idle hooks | fixed |
| 8 | Retain restoration state for removed niri outputs | implemented; compositor checks pending |
| 9 | Move bounded niri IPC off the Wayland loop | fixed |
| 10 | Cancellable, bounded PAM helper processes | fixed |
| 11 | Remove complex post-PAM fork child work | fixed |
| 12 | Pin emergency unlock targets with pidfds | fixed |
| 13 | Validate lock state through one open descriptor | fixed |
| 14 | Honor PAM-requested authentication delay | fixed |
| 15 | Account throttling after PAM identity mapping | fixed |
| 16 | Audit successful starts after launch confirmation | fixed |
| 17 | Audit termination separately from cleanup failure | fixed |
| 18 | Move control-bus maintenance off the actor | fixed |
| 19 | Release transport mutex during async reconnect | fixed |
| 20 | Scale reference restoration within bounded work | fixed |
| 21 | Roll back definitely failed app registrations | fixed |
| 22 | Roll back definitely failed start references | fixed |
| 23 | Recover generation-owned quit/logout helpers | fixed |
| 24 | Bound shutdown concurrency and polling | pending |
| 25 | Linear process snapshot/pidfd signaling | fixed |
| 26 | One recovery deadline across all teardown phases | fixed |
| 27 | Establish shutdown backstop before child startup | fixed |
| 28 | Recover console log level after greeter crashes | fixed |
| 29 | Establish sane terminal baseline after crashes | fixed |
| 30 | Generic autovt ownership | pending |
| 31 | Synchronize generic service and configured VT | pending |
| 32 | Avoid disabled-DM service restart loops | fixed |
| 33 | Reuse render buffers and avoid unnecessary frames | fixed |
| 34 | Cache scaled wallpaper | fixed |
| 35 | Recover output-power changes after locker crashes | pending |
| 36 | Harden remembered-state reads | fixed |
| 37 | Harden privileged log file opens | fixed |
| 38 | Avoid console font temporary-file collisions | fixed |
| 39 | Reap closed generation lease files safely | fixed |
| 40 | Bound coordinator request backlog | fixed |
| 41 | Bound launch request payloads | fixed |
| 42 | Clarify/update Arch release package versions | fixed |
| 43 | Verify downloadable Arch package artifacts | fixed |
| 44 | Narrow release token write permissions | fixed |
| 45 | Pin third-party actions to immutable commits | fixed |
| 46 | Pass only required reusable-workflow secrets | fixed |
| 47 | Pin AUR SSH host identity | fixed |
| 48 | Add Clippy to CI | policy exception |
| 49 | Choose one deterministic automatic keyring | fixed |
| 50 | Follow Desktop Entry Exec parsing semantics | fixed |
| 51 | Reject ambiguous fixed-session display names | fixed |
| 52 | Use logind power capabilities for lock UI | fixed |
| 53 | Validate ready timeout representability | fixed |
| 54 | Document pidfd emergency unlock protection | fixed with 12 |
| 55 | Match bounded recovery-record documentation | fixed with 5 |
| 56 | Match bounded-worker architecture claims | in progress; maintenance and finalize publication off actor |
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

- 11: the post-PAM launcher no longer forks a child that runs Rust/libc setup.
  It serializes the validated session command, user identity, environment, and
  wrapper into a bounded sealed memfd, then starts a fresh `session-exec`
  process image. That helper performs the credential, group, environment,
  directory, and exec operations after a fresh loader has replaced the
  post-PAM image. The existing start gate remains closed until the PAM-owned
  shutdown inhibitor attempt completes. Requests require all four memfd seals,
  reject unknown TOML fields, NUL/invalid environment names, malformed
  commands, and payloads over 1 MiB. The descriptor handoff is close-on-exec
  by default and only the two explicitly cleared descriptors reach the helper.
  Private regressions cover sealed round-trips, mutable-file rejection,
  request bounds, invalid payloads, gate ordering, and parent-loss behavior.
  Full workspace tests, Rust 1.88 all-target checking, and the DM lifecycle VM
  passed; the VM exercises the real DM launcher through Sway, user-manager
  reexec, user-bus restart, NixOS switch, logout, and PAM/inhibitor cleanup.
  The helper remains an internal hidden CLI entry and performs no configuration
  or logging initialization before consuming its validated descriptors.

- 27: the launcher forks its child behind a private close-on-exec start pipe.
  Only close/read/errno operations run at that barrier. The parent establishes
  its PAM delay guard after fork, then releases the child to set credentials,
  prepare its environment, and execute the session. This closes the startup
  window without creating a D-Bus executor before fork. Connection setup and
  inhibitor acquisition share one five-second timeout. EOF or an invalid start
  token aborts the child; a failed parent release reaps it before returning.
  Existing behavior when logind cannot grant the inhibitor is preserved and
  documented; the user coordinator still attempts its independent guard.
  Four private fork regressions cover waiting, parent loss, malformed tokens,
  and close-on-exec descriptors. The DM-lifecycle VM checks the root inhibitor
  in the first compositor commands and confirms no RSDM inhibitor remains
  after logout/PAM closure. It also passes NixOS switch, user-manager reexec,
  user-bus restart, DM restart, and normal app-save/logout checks.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  500 tests (four child-only fixtures are ignored in parent runs).
  Workspace/all-target checks on Rust 1.88.0, `git diff --check`, and
  `nix build .#checks.x86_64-linux.dm-lifecycle --no-link --print-build-logs`
  passed. Changed source files remain below 300 lines and functions below 50.
  Complex post-PAM credential/environment work is still tracked separately
  in finding 11; the start barrier does not claim to remove that limitation.

- 26: recovery retains one absolute budget across connection setup, name
  acquisition, worker drain, application preparation, and every teardown call.
  Application/target/anchor cleanup uses an earlier view to leave time for the
  recorded compositor invocation. Failure to query activation-environment
  ownership now skips shared-target cleanup and preserves the failure while
  still attempting the generation-owned compositor, instead of returning
  before that attempt. An unconfirmed teardown never marks the generation
  closed or extends the saved deadline. Recovery deadline/name-lease regressions
  and the session-lifecycle crash-recovery VM passed, together with all 496
  workspace tests (four child-only fixtures are ignored in parent runs).
  Workspace/all-target checks on Rust 1.88.0 and `git diff --check` passed.
  The changed recovery file and its functions remain below the size limits.

- 1: graceful app preparation, escalation, quit-helper release, and final
  app/logout/target cleanup use earlier views of the same absolute deadline.
  Preparation leaves up to two seconds for escalation and teardown; forced
  application shutdown and infrastructure cleanup leave up to one second for
  the final compositor stop. A phase starting with less time reserves at most
  half its remaining budget. Installing, shortening, or revoking a logind
  deadline also affects views created before the shutdown notice. An expired
  graceful phase leads to escalation while the absolute budget still permits
  ownership verification and signals, rather than attempting KILL through an
  already expired manager. New per-app preparation leases use the phase limit.
  Deadline regressions cover revisions, revocation, late installation, and
  short budgets; a real contended private flock checks the teardown reserve.
  The power-lifecycle VM checks actual app/compositor termination and measures
  inhibitor release on its private logind bus within five seconds, including
  a stubborn app and a hung logout helper. Session-lifecycle also passed,
  covering cancellation, XSMP, teardown ordering, and crash recovery.
  `nix develop --command cargo test --workspace --locked --quiet`, workspace/
  all-target checks on Rust 1.88.0, and `git diff --check` passed. VM commands:
  `nix build .#checks.x86_64-linux.power-lifecycle --no-link --print-build-logs`
  and `nix build .#checks.x86_64-linux.session-lifecycle --no-link --print-build-logs`.
  Changed source files remain below 300 lines and functions below 50 lines.
  A bus that cannot confirm teardown before expiry still produces a failure
  and retains recovery records; the finite budget is never extended to hide
  that failure. Already accepted systemd jobs can continue after it expires.

- Adjacent shutdown defect: signal escalation reads `ControlGroup` from the
  systemd Service interface. Reading it from Unit fails against the real user
  manager with UnknownProperty and prevents both TERM and KILL escalation.
  The private process fixture now exposes the property only on Service, as
  systemd does; the process-signaling regressions cover the correct interface
  while retaining invocation, pidfd, and cgroup membership checks.
  `nix develop --command cargo test -p rsdm-infra session_manager::processes
  --locked --quiet` passed nine tests; infrastructure/all-target checking
  passed on Rust 1.88.0, and `git diff --check` passed. Both changed source
  files remain below 300 lines, with functions below 50 lines.

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

The battery restriction was lifted on 2026-10-07. Earlier syntax-only evidence
below records the checks available when each change was committed; the runtime
verification below supersedes those pending Rust checks. Keep compositor and VM
verification separate from private-peer and unit-test evidence.

- 2026-10-07: `nix develop --command cargo test --workspace --locked` passed
  all 360 tests (one additional isolated subprocess fixture remains ignored by
  default). `nix develop --command cargo check --workspace --all-targets --locked`
  passed without Rust warnings. This executes the new lease, pidfd, record,
  transport, control-bus, audit, logging, font, session-selection, configuration,
  and logind-capability regressions for findings 2, 12, 13, 17, 18, 19, 25, 36,
  37, 38, 40, 41, 51, 52, 53, and 54. The deadline foundation tests also pass;
  findings 1 and 26 remain open for the other teardown phases and VM evidence.
  Finding 8 still needs compositor evidence. Generated-PAM verification for
  finding 49 is recorded below. No host services were restarted or power
  operations requested.

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
- 46: the AUR workflow receives only its declared SSH key and optional commit
  author fields; secrets:inherit was removed. YAML parsing and caller/callee
  secret-contract checks passed locally, without running or publishing anything.
- 45: all external workflow actions use full commit SHAs resolved from their
  original upstream tags/branches (annotated tags use the peeled commit).
  Rust toolchains remain explicitly stable and 1.88.0. Dependabot proposes
  weekly action-pin updates. Upstream refs/action inputs and workflow YAML were
  checked without executing CI; the CI policy remains unchanged.
- 53: configuration validation rejects readiness timeouts that cannot fit an
  Instant deadline, including when coordination is disabled; zero remains valid.
  The launch-time guard remains. Existing config tests moved beside validation
  to keep the production file small; boundary and TOML-loading regressions were
  added. Rust syntax and sample TOML checks passed; runtime tests are pending.
- 51: fixed-session selection prefers exact desktop IDs, then permits only a
  unique Name/Exec match. Duplicate names, commands, and cross-field collisions
  fail with an ID hint; unmatched literal commands retain their behavior.
  Selection lives in the Greeter's session module. Regression tests cover order,
  collisions, metadata preservation, and command fallback; syntax/TOML/diff
  checks passed, with runtime execution pending under battery restrictions.
- 49: NixOS auto keyring detection enables GNOME Keyring when detected,
  otherwise KWallet; it cannot enable both by itself. Explicit gnome/kwallet/none
  choices remain authoritative and RSDM's own PAM services remain excluded.
  Module regressions cover neither, each, both, standalone GNOME, explicit
  overrides, and a Lock PAM override. Nix syntax/TOML/diff checks passed;
  full module evaluation and generated-PAM validation remain pending.
- 47: AUR publishing installs the verified Ed25519 public host key instead of
  trusting a fresh ssh-keyscan result. Strict checking uses only that host file
  and algorithm. Its SHA256 matches the
  [official Arch announcement](https://archlinux.org/news/aur-migration-new-ssh-hostkeys/).
  YAML/shell/SSH configuration parsing and fingerprint checks passed locally;
  no authenticated AUR connection, workflow execution, or publication occurred.
- 36: remembered state opens without symlink following or FIFO blocking,
  validates regular-file ownership/private permissions through the same
  descriptor, and bounds reads/writes to 4 KiB. Missing files remain empty state;
  unsafe files are errors rather than silently accepted data. Tests cover
  descriptor/path replacement, symlinks (including dangling links), FIFOs,
  directories, shared permissions, exact size, oversized data, and writer
  round-tripping. Syntax/diff checks passed; runtime checks remain pending.
- 38: console font snapshots use random 128-bit temporary names with exclusive
  creation and bounded collision retries. Publication synchronizes the file
  before rename and the parent afterward; failed publication removes its own
  temporary file. Tests cover stale PID files, distinct concurrent temporaries,
  replacement/permissions, and failed-rename cleanup in private directories.
  Existing codec tests are preserved. Syntax/diff checks passed; runtime checks
  remain pending, and `/run` snapshots remain intentionally volatile.
- 41: control requests bound argument/environment counts, individual strings,
  encoded quit-command size, and full D-Bus body size before queue admission.
  Generation/action validity and NUL rejection cover the remaining public
  methods too. CLI launch/finalize and app registration share the checks.
  Boundary, encoding-overhead, Unicode/NUL, and private-peer rejection tests
  were added; syntax/diff checks passed, with runtime execution pending.
- 40: bounded control queue plus shared admission permits bound unfinished
  requests across authorization, queued startup, workers, pending replies, and
  stop delivery acknowledgements. Reply clones keep their permit even if the
  caller disconnects. Separate stop/cancel/XSMP slots survive launch saturation;
  excess calls fail with LimitsExceeded without blocking the endpoint executor.
  Tests cover overload, queue saturation, permit lifetime, slot reuse, and
  lifecycle reservations. Syntax/diff checks passed; runtime checks are pending.
- 17: RunningSession separates child wait from recovery. Login audits a known
  exit before recovery can fail or stall, then records recovery/PAM closure as a
  separate cleanup outcome. Unknown waits do not fabricate a finished event;
  failures still retain the shutdown inhibitor through PAM close/drop.
  Ordering tests cover success, recovery/PAM failures, unknown wait, and failed
  or signaled exits. Syntax/diff checks passed; runtime execution is pending.
- 8: Removing a Wayland output no longer drops its connector from the locker's
  restoration obligations. The existing output-on path retains failed restores
  for retry and runs on policy/primary changes and ordinary exit. Syntax/diff
  checks passed; a real niri two-output check is pending. Bounded asynchronous
  IPC and restoration after hard crashes remain tracked separately as 9 and 35.
- 37: Log files are opened relative to pinned, owner/permission-checked parent
  descriptors. Paths cannot traverse symlinks or parent components; leaf opens
  use NOFOLLOW/NONBLOCK/CLOEXEC and verify regular-file type, effective ownership,
  and absence of shared write access. Trusted sticky ancestors are allowed only
  above a private parent. Tests cover append, creation, links, directory replacement,
  unsafe modes/ancestors, FIFO/type checks, and root-only foreign ownership.
  Rust syntax, sample TOML, and diff checks passed; runtime tests are pending.
- 52: Lock uses bounded background CanHibernate/CanSuspend queries to logind
  rather than advertising kernel sleep states. Both footer hints and keyboard
  activation follow the result; yes/challenge are available and errors remain
  unavailable. Connection setup and calls share a five-second deadline. Tests
  cover private peer replies, denied/unknown capabilities, hung connections or
  methods, and independent footer visibility. Syntax/diff checks passed;
  runtime tests and a real logind/polkit check are pending.
- 1 (unit job layer): Each start/stop operation uses one deadline covering
  preflight reads, signal subscription, the mutation reply, and verification of
  the exact job or terminal unit state. Lost replies/signals are reconciled
  inside that budget, with no additional five-second wait and no replay.
  Private-peer tests cover slow preflight/replies/state reads, missing signals,
  zero budgets, and preserving accepted mutations on timeout. Syntax/diff checks
  passed; runtime tests are pending. Sharing a shutdown deadline across all
  calls, worker draining, leases, and child waits is still outstanding.
- 1 (shared bus deadline): All clones of UserManager share a revisable monotonic
  deadline with broadcast wakeup, bounding reads/reconnect, start/stop, unref,
  manager/activation environment updates, XSMP requests, and native delegation.
  ShutdownControl shares that source during coordinator, recovery, and stop-hook
  work; revocation clears it without cancelling unrelated operations. Tests
  cover all-waiter wakeup, shortening/revocation, no polling after expiry,
  watcher lifetime, shared manager clones, and interruption of an existing read.
  Syntax/diff checks passed; runtime execution is pending. Leases, child waits,
  recovery setup/draining, and end-to-end shutdown checks remain outstanding.
- 1 (record lease layer): App record acquisition uses the shared deadline for
  launch/invocation pinning, preparation claims, and cancellation reset. Both
  flock retries and their sleeps observe budget revisions/revocation; normal
  metadata updates retain the independent 250 ms contention cap. An expired
  shutdown cannot create a new lease file. Tests cover contention, expiry,
  revision, revocation, and subsequent reuse. Syntax/diff checks passed; runtime
  execution, child waits, recovery setup/draining, and full lifecycle checks
  remain pending.
- 1 (launcher child layer): Native/external launcher TERM grace and post-KILL
  reaping share the deadline, including revisions while already waiting. An
  expired budget still sends KILL through the original owned pidfd, performs
  only a nonblocking reap, and reports unconfirmed cleanup instead of blocking
  in Child::wait. Five isolated-child regressions passed with
  `nix develop --command cargo test -p rsdm-infra session_manager::session_process --locked`.
  Production/test files have 115/92 lines; no new function exceeds 50 lines.
  Recovery setup/draining and full teardown VM verification remain outstanding.
- 49: `nix eval .#checks.x86_64-linux.module.drvPath` passed the module
  assertions, including all ten keyring cases. The check now inspects generated
  PAM text as well as option values: GNOME/KWallet modules are present exactly
  when expected. A separately evaluated both-detected auto stack contains only
  GNOME Keyring, before sufficient pam_unix authentication and during session
  setup. No system configuration was activated.
- 26 (setup layer): Recovery installs its five-second deadline before opening
  the user bus. Authentication, systemd validation/subscription, bus-name lease,
  and teardown use that same source; recovery cannot extend a saved or active
  deadline. The name lease also has one two-second local cap that includes its
  method replies and retry sleeps. Normal manager setup has a five-second cap.
  `nix develop --command cargo test -p rsdm-infra session_manager:: --locked`
  passed all 120 tests, including hung bus authentication, Version/Subscribe,
  RequestName, shared setup identity, and recovery deadline regressions.
  `nix develop --command cargo check -p rsdm-infra --all-targets --locked` passed.
  Coordinator worker draining, root-helper waits, and lifecycle VM checks are
  still separate outstanding work. Changed Rust files stay below 300 lines.
- 1, 26 (coordinator recovery): Failure recovery clamps the existing manager
  and saved deadlines, publishes that budget before draining workers, and bounds
  request rejection/channel waits by the remaining time. It no longer starts a
  separate 90-second drain or replaces logind's shorter deadline with five new
  seconds. Unconfirmed accepted jobs retain their recorded ownership intent.
  `nix develop --command cargo test -p rsdm-infra session_manager::coordinator_shutdown --locked`
  passed 14 tests, including four private recovery regressions for active/saved/
  expired budgets and completed-work/request handling. Files stay below 300
  lines and new functions below 50. Root-helper waits, app escalation, reply
  acknowledgements, actor reads, and whole-session VM timing remain open.
- 1 (reply delivery): The closed coordinator's final response-acknowledgement
  wait uses the remaining shared shutdown budget instead of adding two fresh
  seconds. The two-second local cap remains for normal logout. Four private
  regressions passed with `nix develop --command cargo test -p rsdm-infra
  session_manager::coordinator_shutdown::tests::replies --locked`, covering
  expiry, remaining time, real endpoint acknowledgement, and queued status.
  Changed source/test files remain below 300 lines; no new function exceeds 50.
- Lifecycle verification attempt: `nix build --no-link --print-build-logs
  .#checks.x86_64-linux.session-lifecycle` failed before VM startup because
  `cache.nixos.org` could not be resolved. No lifecycle result is claimed;
  rerun after DNS/network access is restored. The working host session was
  untouched.
- 1 (ExecStop setup): The app-stop helper reads the saved shutdown deadline
  before constructing its user-manager connection, so a hung bus handshake
  cannot consume a new independent setup budget after expiry. All 128 session
  manager tests passed with `nix develop --command cargo test -p rsdm-infra
  session_manager:: --locked`. The isolated CLI regression
  `nix develop --command cargo test -p rsdm --test session_lifecycle
  expired_stop_hook --locked` passed: both app-stop and recovery reject an
  expired record without connecting to a silent private bus socket or changing
  that record. Rust files remain below 300 lines and new functions below 50.
- 1 (logind request layer): Managed power authorization and the one-shot power
  request use asynchronous operations inside the coordinator's shared deadline.
  Revisions interrupt an already pending system-bus handshake or reply. Ordinary
  callers retain their five-/sixty-second local limits, now covering connection
  setup as well as method completion. No power mutation is retried.
  `nix develop --command cargo test -p rsdm-infra power:: --locked` passed all
  13 private-peer tests, including six new authorization, action routing,
  interactive-request, lost-reply, hung setup/reply, and invalid-action checks.
  No host logind methods were called. Changed Rust files remain below 300 lines
  and functions below 50; actor responsiveness and VM timing remain pending.
- Final local verification of these deadline changes:
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  387 tests; the existing subprocess-only fixture is ignored in the parent
  invocation. `cargo check --workspace --all-targets --locked` also passed under
  the declared Rust 1.88.0 MSRV, using the installed 1.88.0 cargo/rustc inside
  `nix develop`. This includes the Lock preview example and every test target.
  The complete commit-range diff passes `git diff --check`. No changed Rust
  file exceeds 300 lines, and no new function exceeds 50 lines; the existing
  larger coordinator test fixture retains its explicit initialization.
  The lifecycle VM remains unverified because its Nix cache download failed
  DNS resolution. Findings 1, 26, and 56 stay open; no version bump or push.
- Adjacent finalize regression: Repeating finalize after readiness now publishes
  updated manager/activation values and returns success without recreating the
  live anchor or changing phase/ownership. The private-peer regression passed
  with `nix develop --command cargo test -p rsdm-infra repeated_finalize --locked`;
  it verifies both publications and the absence of unit queries/new workers.
  Changed Rust files remain below 300 lines and new functions below 50.
- 56 (finalize publication): One worker serializes manager and activation
  environment updates, with the endpoint admission permit bounding queued and
  active requests. The actor persists ownership before side effects and remains
  the only writer of lifecycle state. Shutdown rejects unstarted publications
  and drains the active one before app preparation; recovery retains its intent
  after partial or unconfirmed completion. A cancelled startup logout preserves
  the accepted finalize until readiness instead of making its launcher exit.
  Eight private-peer regressions cover repeat finalize, a paused publication
  with status/cancel, serialized values, queued shutdown rejection, partial
  failure, deadline-bounded recovery, cancelled startup, and failed record save.
  The coordinator suite passed five consecutive parallel runs after its fixture
  dispatcher race was repaired. Observation/readiness manager calls still need
  to leave the actor, so finding 56 remains open.
  `nix develop --command cargo test --workspace --locked --quiet` passed 395
  tests with one existing subprocess-only fixture ignored in the parent run.
  `cargo check --workspace --all-targets --locked` passed with Rust 1.88.0
  cargo/rustc inside `nix develop`. Architecture documentation now states the
  remaining synchronous-read limitation explicitly. Changed Rust files stay
  below 300 lines; new functions stay below 50. The existing larger startup
  constructor and test fixture retain explicit field initialization.
- Regression fixture readiness: The coordinator's private peer now starts its
  method dispatcher through `Builder::serve_at` before the connection is
  returned. Dynamically registering interfaces did not wait for the match
  stream and could lose the first call, producing intermittent test timeouts.
  The parallel coordinator suite passed five consecutive runs with
  `nix develop --command cargo test -p rsdm-infra
  session_manager::coordinator_shutdown --locked --quiet`. No timeouts or
  assertions were relaxed; the host bus and session were untouched.
- 26 (root-helper exit): Recovery-helper escalation no longer calls an unbounded
  `Child::wait()`. A single local deadline includes a reserved SIGKILL/reaping
  interval; exit is polled nonblockingly and an unconfirmed exit is reported as
  failure. The child stays unreaped until signaling, preventing PID reuse.
  `nix develop --command cargo test -p rsdm-infra unix::session_cleanup --locked`
  passed all four regressions for normal status, stopped-child escalation,
  expired polling and an already reaped child. No host session was involved.
  Root's separate 90-second cap still needs to follow a trusted shutdown budget;
  root must never read a user-owned recovery record to obtain it. Finding 26
  stays open. Changed Rust files stay below 300 lines and functions below 50.
- 1 (notification timing): Logind notices carry the absolute monotonic deadline
  recorded by the monitor, including before its initial shutdown-state query.
  Processing a delayed notice no longer grants a fresh full budget; repeated
  notices retain the earlier limit and cancellation still follows logind's
  authority. Clock failure produces an expired limit rather than no limit.
  `nix develop --command cargo test -p rsdm-infra --locked --quiet` passed all
  239 tests, with one existing subprocess-only fixture ignored in the parent
  run. Two new coordinator regressions preserve queued/expired deadlines;
  private logind tests verify the original budget and both signal values.
  The existing actor-read delays and root-helper budget remain open. Changed
  files stay below 300 lines and new functions below 50.
- 32: Disabled DM configuration now has its own normal runtime outcome and CLI
  exit status 78. Generic and NixOS units accept it with `SuccessExitStatus` and
  suppress its automatic restart with `RestartPreventExitStatus`; other exits
  keep their existing restart policy, including returning from TTY fallback.
  The root example documents the exit contract. All existing CLI tests passed
  with `nix develop --command cargo test -p rsdm --locked --quiet`; the new real
  binary/unit contract test passed with `cargo test -p rsdm --test dm_service
  --locked` inside `nix develop`. It also checks ordinary config validation
  still exits successfully. No host TTY or service was opened/restarted.
  `nix eval .#checks.x86_64-linux.module.drvPath` passed with assertions for both
  generated unit directives. The workspace/all-targets check passed on Rust
  1.88.0 inside `nix develop`. Directive behavior follows the upstream
  [systemd service documentation](https://github.com/systemd/systemd/blob/main/man/systemd.service.xml).
  Changed Rust files remain below 300 lines; new functions below 50. Existing
  longer Greeter orchestration is retained, without growing its responsibilities.
- 29: Initial VT acquisition and reclaim restore a known cooked line discipline
  after checking ownership, before crossterm saves its raw-mode baseline.
  Reclaim now waits out a detached foreign session and reports failures instead
  of proceeding with an unknown terminal state. Metadata errors fail closed.
  The same normalization serves TTY fallback, preserving device speed and
  pending input; service-wide TTY reset/disallocation remains disabled.
  `nix develop --command cargo test -p rsdm-infra unix:: --locked` passed 45
  tests, with the existing child-only fixture ignored in the parent run.
  The new private PTY regression kills its raw-mode owner with SIGKILL and
  verifies canonical input, echo, signals, and CR-to-newline recovery. Separate
  tests check speed preservation and descriptor errors. The live VT was never
  opened or changed.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  403 tests; workspace/all-targets checking passed on Rust 1.88.0. All changed
  Rust files stay below 300 lines, and new functions below 50. The existing
  larger Greeter loop changes only its checked reclaim call. Global console
  log-level crash recovery (28) and generic VT configuration (30/31) remain open.
- 7: Idle hook phases share a 30-second budget, including escalation and
  nonblocking child reaping. Hooks run in a distinct process group; timeout
  signals it before reaping its leader, avoiding numeric group-ID reuse, and
  also kills the owned leader if it moved groups. Timeout skips remaining
  hooks in that phase, while ordinary failure still permits later hooks.
  The existing bounded recovery-child waiter is reused without changing its
  behavior. No dependency or configuration field was added; root config and
  idle/configuration guides explain the foreground-hook budget.
  `nix develop --command cargo test -p rsdm-idle --locked` passed all five
  tests, including timeout followed by a later cycle and ordinary hook failure.
  `nix develop --command cargo test -p rsdm-infra unix:: --locked --quiet`
  passed 48 tests plus the existing ignored child fixture. New command tests
  pin and observe the fixture's stopped leader/shell descendant after timeout,
  preserve nonzero status, and reject expired launches without spawning.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  408 tests; workspace/all-targets checking passed on Rust 1.88.0. No changed
  source file exceeds 300 lines or new function exceeds 50. Locker readiness
  recovery (6) remains a separate pending lifecycle issue.

- 9: One output-power worker serializes niri commands outside Wayland handlers.
  Requests coalesce into one latest topology and a retained restore request;
  stale Off batches stop before their next command. A policy/primary change
  restores completed or uncertain Off commands before applying new work.
  Connector obligations survive output-global removal and failed On replies.
  Each command uses the existing process-group timeout/reaper with a two-second
  budget; each restoration pass shares five seconds. Unlock queues restoration
  without waiting, and App teardown joins bounded cleanup before dropping its
  lock surfaces. No new configuration field or dependency was added.
  Seven regressions cover paused helpers, 1,000 coalesced updates, restoration
  after removed globals/uncertain replies, failed restoration retries, shared
  deadlines, missing executables, and a genuinely stopped child process.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  415 tests; the existing child-only fixture remains ignored in its parent run.
  After the final cleanup-order adjustment, `cargo test -p rsdm-lock --locked
  --quiet` passed all 30 tests inside `nix develop`. Workspace/all-targets and
  final Lock/all-targets checks passed on Rust 1.88.0. No host compositor, output,
  or service was touched. Finding 8 still needs a real two-output compositor
  check; recovery after SIGKILL/crashes (35) is separate and remains open.
  New source files stay below 300 lines and functions below 50. The existing
  302-line Wayland orchestration module shrank by one line; its larger run
  initializer remains necessary to assemble protocol state in one place.

- 6: A locker that misses the ten-second readiness handshake is terminated
  before the idle worker returns. Service-managed lockers first receive a
  bounded `systemctl --user kill --signal=SIGKILL` for their private scope;
  the launcher's private process group is then killed and reaped within the
  shared one-second cleanup budget. A background
  reaper handles an unusual uninterruptible child without holding the idle
  cycle active. A stopped shell and its sleeping descendant must both close
  their inherited connection after timeout. Separate fixtures check the scope
  kill arguments and a hung control command, and the activity-guard test uses
  the real worker guard. `nix develop --command cargo test -p rsdm-idle -p
  rsdm-infra --locked --quiet` passed all seven idle tests and the infrastructure
  tests. No host service or compositor was contacted. Crashing a locker after
  an unreported protocol lock retains the compositor's fail-secure behavior;
  cleanup does not send an unlock request.
  All-target checking of Idle and infrastructure passed with Rust 1.88.0;
  `git diff --check` passed. Changed source files remain below 300 lines. The
  existing lock-cycle orchestration remains above 50 lines and now uses the
  same activity guard as its regression test.

- 28: Greeter console logging now has its own module. Before changing printk,
  it atomically publishes the original four levels in a private, bounded
  `/run/rsdm/console-printk.state` file and syncs the file and directory. A new
  Greeter reuses that baseline after a crash rather than remembering the quiet
  level. Normal exit restores the exact original values before removing the
  snapshot; failed restoration retains it for another attempt. Unsafe files,
  symlinks, shared ownership, and unavailable persistence prevent quieting.
  A nonblocking process-associated lease serializes Greeters without leaking
  ownership to the detached PAM/session process across fork. PID 1 status
  output retains its existing enable-on-return behavior; a recovered snapshot
  also triggers that compensation before the new Greeter quiets it again.
  A real SIGKILL subprocess regression verifies crash recovery, separate
  process fixtures cover contention and fork inheritance, and file fixtures
  cover normal exit, unsafe state, malformed/oversized records, and failed
  restoration. `nix develop --command cargo test -p rsdm-tui --locked --quiet`
  passed all 17 tests; three isolated fixtures are invoked by their parent
  tests. No kernel setting or PID 1 was touched by these checks. Recovery runs
  at the next Greeter entry; an intentionally stopped service needs a later
  entry or manual restoration. All changed source files stay below 300 lines
  and new functions below 50 lines. TUI/all-target checking passed with Rust
  1.88.0; `git diff --check` passed.

- 34: Each lock surface owns one prepared cover-fit image for the immutable
  wallpaper loaded at startup. Physical buffer-size changes invalidate it;
  dim/animation/text zoom changes copy the prepared pixels and apply effects
  afterward. Resizing frees the obsolete cache before allocation, and allocation
  failure falls back to the original resampler rather than suppressing a frame.
  Output removal naturally drops its cache; no global size-keyed map accumulates.
  Five headless regressions cover exact crop/opaque pixels, dynamic dim without
  cache corruption, independent output sizes/resizing, empty images, and animated
  composition with zoom changes. An initially incorrect rectangular fixture
  expectation was corrected against the existing center-crop calculation;
  the resampling algorithm remains unchanged.
  `nix develop --command cargo test -p rsdm-lock --locked --quiet` passed all
  35 tests. `nix develop --command cargo test --workspace --locked --quiet`
  passed all 420 tests, plus the existing child-only fixture ignored in the
  parent run. Workspace/all-targets checking passed on Rust 1.88.0.
  `git diff --check` passed. New source and functions stay below 300/50 lines.
  The existing Wayland module is 303 lines: one cache field belongs beside its
  per-output surface/buffer state. Existing larger rendering and startup
  orchestration is retained. Persistent frame-buffer reuse remains separate (33).

- 33: Each surface retains a reusable software canvas and at most two SHM
  buffers. Compositor-owned buffers remain immutable until wl_buffer.release;
  old-size active buffers count against the same limit during resize. When all
  buffers are busy, draw skips painting/copying and retains the dirty request
  for a later poll. Stable black secondary geometry is not submitted again.
  Canvas reset clears the previous frame, and checked resize preserves existing
  pixels after allocation failure. SHM selection/commit lives separately from
  pure software composition; the enlarged draw method was reduced below 50 lines.
  Five new regressions verify canvas reuse/reset/failed resize, actual Release
  events on a private Wayland socket, unchanged buffer IDs/data across 100 reuse
  calls, no pool growth across 999 busy geometry changes, and replacement of
  released obsolete geometry. No host display or service was accessed.
  `nix develop --command cargo test -p rsdm-lock --locked` passed all 40 tests.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  425 tests, plus the existing child-only fixture ignored in its parent run.
  Workspace/all-targets checking passed on Rust 1.88.0; `git diff --check` passed.
  New modules/functions remain below 300/50 lines. The existing Wayland module
  is 307 lines: its per-output resource ownership and dirty-retry orchestration
  belong together. Its pre-existing larger run initializer and event loop remain.
  One software-to-SHM copy per submitted frame is retained; buffer allocation and
  repeated static-black submissions are removed. No performance benchmark or
  real multi-output compositor run is claimed. Static-background clock refresh
  was observed as a separate existing issue and still needs its own fix.

- Rendering follow-up: the clock previously updated only on another dirty
  event when the background was static. The Wayland loop now compares the
  displayed clock snapshot during its normal poll cycle and marks the frame
  dirty only when that text changes. Primary rendering uses the same snapshot;
  animation and buffer-release retries retain their existing behavior.
  This small UI scheduling fix uses the existing regression suite rather than
  adding a test that duplicates a string comparison. `nix develop --command
  cargo test --workspace --locked --quiet` passed all 425 tests, plus the existing
  child-only fixture ignored in its parent run. Workspace/all-targets checking
  passed on Rust 1.88.0; `git diff --check` passed. No host clock/display was
  changed; a live static-lock minute transition was not exercised.
  The existing Wayland module is 310 lines; its clock snapshot belongs beside
  other frontend state. Its pre-existing larger run initializer/event loop
  remain. The new clock operation is eight lines.

- Output-worker follow-up: final cleanup now supersedes an in-flight restoration
  pass between commands, so it waits for at most the current bounded command
  before its own shared restoration budget. The former pass could otherwise
  finish its whole budget before cleanup began. A retained worker-local restore
  request preserves remaining obligations when topology updates coalesce during
  interruption; no stale Off work resumes before restoration completes.
  Two deterministic paused-command regressions check that Shutdown is consumed
  before the next On, and that an interrupted restore finishes before newly
  queued outputs are powered off. `nix develop --command cargo test -p rsdm-lock
  --locked` passed all 42 tests. `nix develop --command cargo test --workspace
  --locked --quiet` passed all 427 tests, plus the existing child-only fixture
  ignored in its parent run. Workspace/all-targets checking passed on Rust
  1.88.0; `git diff --check` passed. Both changed source files remain below
  300 lines and all changed functions below 50. Tests use only private worker
  channels; no host compositor, output, service, or clock was changed.

- 22: Stage AddRef intent only after proxy/signal preparation, immediately before
  the mutation. Each attempt settles its own claim; overlapping rejected attempts
  remove an unowned entry only after the last settles, while earlier/accepted
  ownership and invocation identity remain intact. Claims cannot remove a later
  replacement entry after forget/re-register. Lost replies, cancelled waits,
  post-acceptance read errors, and partially created units retain recovery intent.
  A published replacement bus peer may already hold a restored reference; the
  rejection reports that obligation instead of silently forgetting it.
  Start errors distinguish pre-submission failure from direct UnitExists and
  validated pre-creation rejections. Other explicit validation/authorization
  errors require an authoritative NoSuchUnit read within the original budget;
  they are never inferred from inspection errors after an accepted mutation.
  This follows upstream systemd's ordering of transient properties, AddRef, and
  job creation in [dbus-manager.c](https://github.com/systemd/systemd/blob/v250/src/core/dbus-manager.c).
  Twelve private-bus/claim regressions cover rejection, ownership overlaps,
  replaced entries, real candidate-peer publication, partially created units,
  accepted read denial, lost replies/timeouts, and preparation failures.
  Existing job fixtures were separated by responsibility; their standard D-Bus
  error names remain exact rather than being wrapped as generic zbus errors.
  `nix develop --command cargo test -p rsdm-infra session_manager::bus --locked
  --quiet` passed all 40 bus tests. `nix develop --command cargo test --workspace
  --locked --quiet` passed all 439 tests plus the existing child-only fixture
  ignored in its parent run. Workspace/all-targets checking passed on Rust
  1.88.0; `git diff --check` passed. All changed source files remain below 300
  lines and new functions below 50. No host bus/session/service was touched.
  Application-record rollback (21) consumes this typed evidence in the separate
  implementation below. Partial or unconfirmed ownership is intentionally
  retained for ordinary cleanup/recovery.

- 21: Application launch rolls back its unpublished invocation record after
  local preparation failure or a definitely rejected start. Rollback holds only
  a short bounded record lease and preserves records already claimed by another
  invocation. A replacement peer's retained reference keeps the recovery record.
  A rejected start never reads or pins a conflicting unit's invocation; other
  starts validate the generation and stable invocation before publication.
  Lost replies, partial creation, and post-acceptance inspection failures retain
  recovery state without replaying StartTransientUnit. Cleanup contention or
  file-system failure reports both errors rather than claiming successful rollback.
  Nine private-peer/record regressions exercise these paths, including cleanup
  retry after contention and ordinary release of a partially created unit.
  `nix develop --command cargo test -p rsdm-infra session_manager::apps:: --locked`
  passed all nine tests; `nix develop --command cargo test --workspace --locked
  --quiet` passed all 448 tests plus the existing child-only fixture ignored in
  its parent run. Workspace/all-targets checking passed on Rust 1.88.0;
  `git diff --check` passed.
  No live user manager, service, or session is contacted. All changed Rust files
  remain below 300 lines and new functions below 50.

- Helper-start follow-up: quit and logout cleanup now consumes the same typed
  rejection evidence. A definitely rejected or unsubmitted start never issues
  StopUnit/UnrefUnit against a conflicting helper name. Accepted starts whose
  subsequent inspection fails retain the existing cleanup attempt. A replacement
  peer's uncertain retained reference is not treated as permission to stop that
  name; generation helper recovery remains open under finding 23.
  Four private-peer regressions cover both helper callers, real UnitExists and
  AccessDenied replies, and post-acceptance inspection errors. No command is
  executed by the fixture and no live manager/session is contacted.
  `nix develop --command cargo test -p rsdm-infra helper_ --locked --quiet` passed
  all six matching tests; `nix develop --command cargo test --workspace --locked
  --quiet` passed all 452 tests plus the existing child-only fixture ignored in
  its parent run. Workspace/all-targets checking passed on Rust 1.88.0;
  `git diff --check` passed. All changed Rust files remain below 300 lines and
  all new functions below 50. The existing coordinator test constructor exceeds
  50 lines because it assembles its isolated coordinator state; it was unchanged.

- 39: Closed-session cleanup now scans only per-app `.lock` files whose matching
  record is gone. It opens each file with `O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK`,
  validates owner/type/mode, and removes it only after acquiring a nonblocking
  exclusive flock. A held inode is retained for its owner and retried during a
  later closed-generation cleanup; no lock is unlinked while another operation
  can still use it. Normal coordinator cleanup and recovery both run this pass
  after application records are finished. Private runtime tests cover records
  that still exist, stale lock removal, and a contended lock retained until its
  holder exits. No host runtime directory is accessed.

- 23: Transient quit and logout helpers now carry both `PartOf` and `Requisite`
  references to the generation anchor. They cannot start after the anchor has
  gone, and systemd stops them when the anchor is stopped. Recovery already
  stops the recorded anchor before closing the generation, so a coordinator
  crash cannot leave a helper independent of the session lifetime. Rejected
  starts still avoid StopUnit/UnrefUnit on a conflicting name (covered by the
  helper-start tests above). Private D-Bus regressions inspect both lifetime
  properties for quit and logout helpers and exercise rejected and
  post-acceptance failures. No live manager or session is contacted.

- 42/43: Both Arch recipes now track the latest published upstream release
  `v1.1.0`, rather than the obsolete `1.0.0` template. The source recipe uses
  the downloaded tag archive's SHA-256, and `rsdm-bin` uses the SHA-256 digests
  published for both x86_64 and aarch64 release archives. No `SKIP` checksum
  remains. The release metadata was checked against the
  [upstream v1.1.0 release](https://github.com/Frysuni/rsdm/releases/tag/v1.1.0);
  local `bash -n` checks pass for both PKGBUILDs and the install script. The
  current workspace version is a development state beyond that published
  release, so these recipes intentionally remain tied to an existing download
  until a matching release is published.

- 15: LoginAttemptLimiter now receives the directory resolver's canonical
  username before admission, so aliases that map to one account share a failure
  bucket. UnixUserResolver derives that key from the same reentrant passwd lookup
  used for account resolution. If canonical lookup fails, authentication keeps
  its previous behavior and the submitted name is used for limiting; this avoids
  turning an identity lookup failure into a new authentication outcome. Successful
  limiter updates use the same canonical key, while PAM audit/session identity
  remains the name returned by PAM.
  A core login regression maps `ALICE@example` to `alice` and verifies both the
  admission and success events use `alice`. Existing resolver implementations
  retain a default identity method. No live PAM, NSS, user session, or limiter
  state is touched by the test.
  The actual Greeter's parent limiter now also tracks submitted names plus
  directory/PAM identities reported through the existing conversation pipe.
  The child reports its name before authentication and after authentication
  and account management, including failures; the parent acknowledges the
  mapping only when its budget allows it. Rejection cannot be bypassed by a
  later authorization frame or a cancelled conversation. All counters remain
  in the parent, and repeated reports do not count one failure more than once
  for the same identity. Directory lookups stay inside the PAM owner. Mappings
  unavailable before authentication can still require PAM work to discover;
  an exhausted mapped account cannot open a session. The account payload is
  bounded before it is read, tracked identities are bounded per attempt, and
  PAM's failure status is preserved when its account item is inspected.
  Parent-limiter regressions rotate aliases through failed attempts, check
  de-duplication and successful reset, and preserve unrelated accounts and
  failures after a lost helper. Private protocol tests cover both acceptance
  and rejection, cancellation, failed authentication, and invalid payloads.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  491 tests, with four subprocess fixtures ignored in their parent runs and
  executed by the corresponding tests. Workspace/all-target checking passed
  with Rust 1.88.0; `git diff --check` passed. Changed source files stay below
  300 lines. The existing Greeter submit callback remains above 50 lines; its
  limiter/conversation state is isolated in the new attempt module.

- 14: PamHandle no longer installs a no-op `PAM_FAIL_DELAY` callback. Linux-PAM
  and its configured modules retain ownership of the requested authentication
  delay, including module-specific aggregation and randomization. RSDM's
  username limiter remains an additional admission control and no longer
  overrides the PAM stack's delay policy. The change removes the unused FFI
  constant and callback; PAM setup/authentication tests still pass without
  contacting a real authentication service.

- 10: Lock-screen PAM verification now runs in a dedicated hidden helper image,
  with a bounded framed Unix-socket protocol for prompts, answers, cancellation,
  and results. The parent retains limiter state, clears the helper environment,
  passes only an explicitly inherited socket descriptor, applies a 90-second
  monotonic timeout, and kills/reaps the helper on cancellation, timeout,
  malformed protocol data, or transport loss. The helper has a parent-death
  signal and never receives the password in argv. Protocol regressions cover
  bounded secrets, malformed lengths, and partial frames; lock tests pass
  without contacting PAM.
  Greeter session-leader authorization now uses the same 90-second deadline for
  its handshake and framed PAM report reads. A timeout, cancellation, lost
  report, or invalid authorization terminates and reaps the direct child before
  returning to the UI. Interactive Greeter prompts also enforce the deadline,
  while Escape and the existing termination flag still cancel user-driven
  prompts. Infra and TUI tests cover timeout behavior and the existing
  cancellation/account-mapping protocol.

- 50: Session discovery decodes the desktop string escape layer, validates whole
  double-quoted arguments, and expands Exec field codes after tokenization.
  Unknown/malformed codes and invalid quoted or multi-argument placements are
  rejected. File arguments are absent when starting a session; deprecated codes
  disappear, and more than one file/URL code is invalid. The previous stripping
  fixture contained two such codes; the accepted case now uses one and explicit
  rejection regressions cover duplicates.
  Name, source path, and optional Icon expand without splitting whitespace or
  recursively expanding percent sequences. Name/Icon select locale variants in
  the specified country/modifier/language order, with the required default.
  Escaped Name whitespace remains intact. The resulting argv is encoded in the
  existing launcher's command syntax; this final encoding is lossless transport,
  not a second Desktop Entry interpretation. Literal configured commands retain
  the existing launcher syntax. No new dependency or config option is needed.
  Implementation follows the upstream [Exec rules](https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html),
  [string escapes](https://specifications.freedesktop.org/desktop-entry/latest/value-types.html),
  and [locale matching](https://specifications.freedesktop.org/desktop-entry/latest/localized-keys.html).
  Seventeen new regressions cover both escape layers, every reserved character,
  field replacements, localized metadata, malformed entries, lossless argument
  encoding, discovery across invalid files, and actual PreparedCommand argv with
  a session wrapper. Tests use private files or in-memory command preparation;
  they never launch a desktop, contact a user manager, or modify host locale.
  `nix develop --command cargo test --workspace --locked --quiet` passed all
  469 tests plus the existing child-only fixture ignored in its parent run.
  Workspace/all-targets checking passed on Rust 1.88.0; `git diff --check` passed.
  All changed Rust files remain below 300 lines and all new functions below 50.
  A real desktop launch is not claimed by these argument/discovery checks.

- 20: User-bus reconnect restores remembered transient-unit references in
  bounded batches of eight concurrent unit checks. Each batch still shares the
  reconnect deadline, and the transport publishes the candidate connection
  only after every batch and activation-environment check succeeds. A failed or
  stale candidate is closed, so partial RefUnit ownership is not published as
  the active transport. The batch width bounds D-Bus work and descriptor/task
  pressure while avoiding the previous one-unit-at-a-time convoy.
  A private peer regression restores 24 references with delayed RefUnit calls,
  observes parallel progress, and verifies the concurrency ceiling. Existing
  reconnect, replacement, stale-publication, and mutex-responsiveness tests
  remain in the same isolated suite. No live bus or session is contacted.
  `nix develop --command cargo test -p rsdm-infra reference_restoration --locked
  --quiet` passed; workspace tests and Rust 1.88 checking are recorded after
  the complete audit batch. `git diff --check` passed. The existing reconnect
  operation retains one five-second deadline; a genuinely unavailable bus can
  still exhaust it, while large healthy reference sets now scale by bounded
  concurrency.
