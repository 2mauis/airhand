# Phase 1 PV investigation

The initial-run notes below are historical. The follow-up section at the end
supersedes the initial Go/client verification blocks; no acceptance criteria
were changed.

Started from `feat/tailcat-p2p` at `b85c730a1c2da15d27fd8ab96f39cebc445b6182`.
The contract is `docs/specs/tailcat-p2p.md` r6.5.2, especially §2 and the
S3-9 rule in the phase table, and `tests/p2p-a-registry.txt`.
No existing acceptance assertions or thresholds were changed. Relay
`net::transport_config()` and all protected workflow, hook and scan files are
unchanged. No D, O, H, session-switching or helper-gate implementation was added.

## Implemented primitives and current disposition

- `p2p/disco.go`: concurrent `Ping` plus a 250 ms pipelined sender, per-call
  contexts, cancellation and sequence-labelled results. It contains no D policy.
  `disco_net.go` supplies a hermetic DERP/STUN and allowlisted tailcat v0.7.0
  client/server pair for the existing #55 test. Explicit noop modes bypass it.
  The only new module dependency tree is the spec-required tailcat v0.7.0 tree;
  its requirement raises the Go version to 1.27.1. The three unavailable module
  checksums were taken from that pinned upstream module's own go.sum. A complete
  successful `go mod tidy` remains blocked by downloads, so this Go integration
  is **not verified**.
- `airhand_link::direct::direct_transport_config()`: an independent copy with
  32 incoming uni streams, 250 ms keepalive and fixed 1200-byte MTU. The relay
  still has zero uni streams and its original settings.
- `direct::probe::ProbeSource`: first probe immediately, then every 250 ms;
  highest priority by default; one-byte stream plus FIN; an independent task
  publishes progress only after `stopped()` returns `Ok(None)`. It polls
  `open_uni` once, skips unavailable credit, caps outstanding tasks at 16, and
  has bounded event channels. Dropping the source cancels its tasks. There is
  no helper watchdog or IPC policy in this module.
- The acceptance glue now uses real Quinn 0.11.12 / quinn-proto 0.11.19 over a
  byte-preserving memory channel. It records actual events, models directional
  loss and FIFO serialization, and retains both noop polarities. A single
  ordered delivery queue prevents timer rounding from inventing packet
  reordering. The #123 measurement alone keeps the connection alive beyond its
  intentional 10 s outage (60 s fixture idle timeout); this is not a gate or an
  acceptance threshold change.
- #121, #122 and #123 are green and were removed from the expected-red list.
  #55 is blocked; #126 and #140 are red; #145 fails fixture validity. Those four
  were retained in the list. D/O/H work is stopped pending the findings below.

## PV-1 (#55)

The independently tested sender scheduling/cancellation passed:

```
cd p2p
go test -race -count=20 disco.go disco_sender_test.go
# ok command-line-arguments 16.101s
```

The test holds calls open, observes at least four concurrent calls, and verifies
that cancellation releases all callers and closes the results channel. This is
**not** evidence that tailcat's real concurrent calls are safe.

The requested real-client `go test -race -count=20 -run
'^TestDiscoPingConcurrentCallsAreSafe$' ./...`, full Go suite and both Go noop
suites were attempted but stopped during setup. Go's approved module proxy
redirects the required gVisor, compression and u-root archives to
`storage.googleapis.com`, which is not allowed by this environment. Module
resolution/tidy consequently failed. Later offline attempts avoided repeatedly
requesting the denied destination and confirmed those archives are absent.
No #55 pass or real-tailcat race/noop conclusion is claimed. This is an
infrastructure block, not a demonstrated library-behavior failure.

## PV-3 (#126): extra RTT root cause

The standalone `examples/pv_raw.rs` imports Quinn, Tokio and Rustls directly.
It does not use any airhand library module, transport factory, probe source or
acceptance glue. The only shared artifact is the explicitly public localhost
TLS test identity. Run:

```
cargo run -p airhand-link --example pv_raw -- handshake
P2P_TRACE=1 cargo test -p airhand-link --test direct_quinn \
  host_handshake_not_abandoned_under_single_loss_up_to_rtt_350ms -- --nocapture
```

At RTT 300 ms the host's first 1200-byte datagram contains coalesced Initial and
Handshake packets (`long=[0, 2]` in the acceptance wire trace). Dropping that one
UDP datagram produces this sequence (times from the client's connection start):

| Event | Time |
|---|---:|
| First Initial reaches host; first host flight is dropped | 150 ms |
| Client's initial PTO sends Initial probes | 999 ms |
| Host's initial PTO sends Initial-only probes | 1149 ms |
| Client receives retransmitted Initial and sends its Initial ACK | 1299 ms |
| Host receives Initial ACK and retransmits Handshake CRYPTO | 1449 ms |
| Client receives Handshake CRYPTO and sends Finished | 1599 ms |
| Host receives Finished and completes | 1749 ms |

Relative to the host's first inbound packet this is **1599 ms = 999 ms +
2 RTT**, independently reproduced without airhand glue. With the specified
50 ms IPC budget it is 1649 ms, beyond the unchanged 1500 ms H.

Quinn tracks retransmission separately for each packet-number space. In
quinn-proto 0.11.19, `connection/mod.rs::pto_time_and_space` iterates Initial
before Handshake and changes the selected space only for a strictly earlier
PTO. When their send times tie in a coalesced flight, Initial wins.
`on_loss_detection_timeout` queues loss probes only in that selected space.
`connection/spaces.rs::PacketSpace::maybe_queue_probe` retransmits CRYPTO only
from that space, not the whole lost coalesced flight. The subsequent Initial
ACK/RTT update enables the Handshake retransmission in the trace. The spec's
one-RTT recovery assumes more of the lost flight is recovered at the first PTO.

Raw host completion times measured from first inbound:

| RTT | Completion | Completion + 50 ms IPC |
|---:|---:|---:|
| 100 ms | 1199 ms | 1249 ms |
| 200 ms | 1399 ms | 1449 ms |
| 300 ms | 1599 ms | 1649 ms |
| 350 ms | 1699 ms | 1749 ms |
| 375 ms | 1751 ms | 1801 ms |
| 400 ms | 1799 ms | 1849 ms |

The 375 ms result includes millisecond timer rounding. The unchanged acceptance
case fails at RTT 300 ms. H implementation is stopped. No `initial_rtt`, H or
acceptance limit was adjusted. The supplied first flight is coalesced; this run
is not evidence about a separately packetized Handshake-only loss variant.

## PV-2 (#140): burst bound also red

The first unchanged assertion fails for a 100 ms bidirectional burst at phase
0, RTT 400 ms, controller-to-host delay 0 ms, ±50 ms per-datagram jitter:

```
Host maximum progress gap: 712 ms
nominal-grid r: 1
existing bound: (r + 1) * 250 + 25 + 50 = 575 ms
burst end -> first progress: 572 ms
```

Independent reproduction:

```
cargo run -p airhand-link --example pv_raw -- burst
```

It produces the same 712 ms gap using raw one-byte `open_uni` / `finish` /
`stopped` calls. Around the burst, host progress occurs at 4.960 s, 5.672 s,
5.755 s and 5.966 s. This establishes that changing the airhand signal-source
wrapper is insufficient to satisfy the existing bound.

The oracle computes missing nominal grid events using a fixed one-way offset;
actual packet/ACK times move with per-datagram jitter and reliable recovery.
The independent run also measures maximum smoothed RTT 424.471 ms: configured
base RTT 400 ms plus jitter is not a guarantee that measured RTT remains within
400 ms (especially the zero-delay leg, whose negative jitter is clipped).
Consequently this is evidence against the supplied bound/fixture combination,
not proof of a Quinn defect throughout the spec's supported RTT envelope.
The current assertion is retained; O/D work is stopped rather than changing the
oracle or making the threshold pass. No later #140 matrix entries were pursued
past the first failed assertion.

## #145: separate saturation investigation

Run the unchanged acceptance case and its independent API-only reproduction:

```
P2P_TRACE=1 cargo test -p airhand-link --test direct_quinn \
  probe_streams_use_highest_send_priority_and_survive_media_saturation_real_quinn \
  -- --nocapture
cargo run -p airhand-link --example pv_raw -- priority
```

The raw experiment uses the specified 20 Mbps link, 250 ms base RTT, 150 ms
FIFO tail-drop queue, continuously written media stream, and default vs maximum
probe priority. In this run both produced 23 host progress events, ending at
5.901 s (6 of 60 one-second buckets), maximum inter-event gap 326 ms, and about
8.46 MB of delivered media. Both connections then close with:

```
INTERNAL_ERROR: too many gaps in stream buffer
```

This is a stronger failure than merely missing one second, but **does not
reproduce the previous attempt's exact 49/60 observation**; its code/logs are not
on this baseline and that observation cannot be attributed to the same cause.

In quinn-proto 0.11.19, `connection/streams/recv.rs` turns an assembler insertion
failure into that transport error. `connection/assembler.rs` bounds fragmented
buffer chunks (`MAX_CHUNKS = 1024` after compaction). Saturating BBR plus tail
drops can exhaust that receive-side fragmentation budget. Probe priority
orders stream data; it cannot repair a connection closed by a media-stream
reassembly error, bypass congestion control, or prevent lower-layer tail drops.
The similar maxima before close do not establish priority equivalence under
sustained valid conditions; the experiment stops too early.

Crucially, the **actual measured smoothed RTT reaches 400.999993 ms**, beyond
#145's unchanged 400 ms validity limit. The acceptance glue now reports this
actual statistic as well as the channel queue bound, instead of treating
configured base delay plus queue as measured RTT. #145 therefore correctly
fails its fixture-validity assertion before the 200-gap positive control.
Neither maximum priority nor the prior 49/60 count proves PV-2 false on the
valid supported path. This item is stopped with a reproducer and an invalid-
fixture finding; no queue, RTT, required-progress or ack-stall threshold changed.

## Test results

Counts are individual test outcomes, not package/build failures. Logs are in
`/workspace/work/logs` in this session; the raw commands above regenerate the
relevant evidence without these logs.

| Suite | Green | Red | Ignored / blocked |
|---|---:|---:|---|
| Root baseline (`cargo test --workspace --locked --no-fail-fast`) | 38 | 93 | 1 ignored |
| Root final, same command | 41 | 90 | 1 ignored |
| Root noop-lenient | 38 | 93 | 1 ignored |
| Root noop-strict | 38 | 93 | 1 ignored |
| Real Quinn PV subset | 3 | 3 | #126, #140, #145 described above |
| Go baseline before adding tailcat | 1 | 38 | 0 |
| Go final/full and real #55 race20 | 0 | 0 | Setup blocked; no tests executed |
| Go sender-only race20 | 20 repetitions passed | 0 | Does not test real tailcat |
| Go noop-lenient / noop-strict final | 0 | 0 | Setup blocked; no tests executed |
| Client Rust (`cargo test --workspace --locked --no-fail-fast`) | 0 | 0 | Build blocked: missing glib-2.0 development files |
| Client frontend (`npm test`) | 53 | 0 | 0 |
| Workflow script checks | 149 | 0 | 0 |

The root expected-red gate passes with exactly 90 assertion failures, 41 green
and 1 ignored. Both root noop gates pass with exactly 93 assertion failures;
#121–#123 become red again under either polarity. Gate self-tests pass 26/26.
The expected-red list now has 138 entries and the registry remains 144.
Frontend `npm ci` and `npm run build` pass. `git diff --check` passes.

Automatic approval review rejected installation of the existing CI Linux
prerequisites because it interpreted the user's dependency restriction as
covering system build packages. Approval was requested for those prerequisites,
and environment access was requested for `storage.googleapis.com`; neither
answer/configuration change was available during this run. Client Rust and real
Go verification remain incomplete. These are not recorded as assertion failures.

Runtime identity disclosure: the available instructions identify this agent as
Codex based on GPT-6, but do not expose the exact served model identifier or
reasoning level. Those exact values cannot be truthfully supplied from this
runtime; no model or reasoning setting is inferred from tool/model menus.

## Follow-up: approved Linux prerequisites, no storage.googleapis.com access

The user approved the existing Linux CI prerequisite packages and required that
this follow-up not access `storage.googleapis.com`, change environment network
settings, add Go/Rust dependencies, alter assertions/the expected-red list, or
commit/push/open a PR. The expected-red list's three deletions from the initial
run were retained byte-for-byte; this follow-up made no list edits.

The escalated system APT installation still failed with filesystem permission
errors. There is no sudo/scoped package installer in this executor. Instead,
APT downloaded 135 existing CI packages/dependencies from Debian's official
allowed HTTPS mirror into `/workspace/work/pv-resume/apt`, validating signed
package indexes and package checksums. `dpkg-deb` extracted them into
`/workspace/work/pv-resume/sysroot`. No system APT configuration or environment
network policy was changed. Workspace-local pkg-config/linker paths select this
installation for the client build. Verified versions: GLib 2.84.4, WebKitGTK
2.54.0, ayatana-appindicator 0.5.94, PulseAudio 17.0.

The missing pinned compress v1.19.1, gVisor
v0.0.0-20260915211658-a6f909f08a72 and u-root v0.14.0 source archives were fetched
from their upstream GitHub codeload URLs. Canonical Go module archives were
reconstructed with the already-cached `golang.org/x/mod` tooling and their
hashes matched the existing repository go.sum exactly. HTTPS requests used no
redirect following; module metadata came directly from proxy.golang.org.
Subsequent Go checks used `GOPROXY=off`, so no Go download could follow a
storage.googleapis.com redirect.

The repository's unmodified Go dependency files still omit the google/btree
v1.1.3 indirect requirement/checksums already present in the pinned upstream
tailcat dependency graph. Therefore the original in-place `go test
-mod=readonly` still stops during setup. To test the actual source without
editing repository dependency files, the checks below use
`-modfile=/workspace/work/pv-resume/p2p.go.mod`. Only that temporary modfile and
sumfile add the existing upstream btree record; its archive was independently
reconstructed from google/btree v1.1.3 and matched the pinned upstream checksum
`h1:CVpQJjYgC4VbzxeGVHfvZrv1ctoYCAI8vbl07Fcxlyg=`. No new dependency/version was
introduced into the upstream graph, and repository go.mod/go.sum were unchanged.
The temporary metadata qualification is essential: this is source/API
verification, not a claim that the original manifests pass directly.

Commands from `p2p/` (Go 1.27.1, offline):

```
go test -modfile=/workspace/work/pv-resume/p2p.go.mod -mod=mod \
  -race -count=20 -run '^TestDiscoPingConcurrentCallsAreSafe$' ./...
go test -modfile=/workspace/work/pv-resume/p2p.go.mod -mod=readonly -json ./...
P2P_TEST_NOOP=lenient go test \
  -modfile=/workspace/work/pv-resume/p2p.go.mod -mod=readonly -json ./...
P2P_TEST_NOOP=strict go test \
  -modfile=/workspace/work/pv-resume/p2p.go.mod -mod=readonly -json ./...
```

The real-tailcat #55 race run passed all 20 repetitions (`ok airhand/p2p
3.418s`), with no race report. Both noop modes fail #55 with the required ASSERT
marker. Both complete Go noop gates pass: exactly 36 top-level assertion
failures, 2 passes, zero skipped, registry self-checks 26/26. After placing both
parsed result files in the meta-test's fixed ignored `target/` locations,
`TestNegativeAssertionsFailOnNoopGate` also passes while reading those results.

| Go check, using temporary metadata | Green top-level | Red top-level | Result |
|---|---:|---:|---|
| Real #55 race20 | 20 repetitions | 0 | Pass |
| Normal complete suite | 3 | 35 | Remaining first-red assertions |
| Noop lenient | 2 | 36 | Expected assertion failures; gate passes |
| Noop strict | 2 | 36 | Expected assertion failures; gate passes |
| Noop-result meta-test | 1 | 0 | Pass |
| Original manifests, readonly | 0 | 0 | Setup blocked by missing btree checksum/record |

Two failing nested subtests (`TestWriteTimeoutReportsPathFailure/tunnel` and
`/pipe`) are excluded from these top-level totals to avoid counting their
already-failing parent twice. The initial report's baseline Go 1/38 raw event
count included those two nested failures; the corresponding top-level baseline
was 1 green / 36 red.

The normal Go expected-red gate now reports precisely one mismatch: #55 passed
while still listed as expected red. The list was not changed, and list-update
work was stopped under the user's instruction. This is a metadata/gate mismatch,
not a failed real-tailcat check. The prior frozen #126/#140/#145 investigations
and all their assertions/thresholds are unchanged.

Client verification now completes with the workspace-local CI prerequisites.
CMake 3.31.6 is on the build PATH; 96 workspace symlinks reference already
installed compatible host runtime libraries omitted from APT's download-only
set because they were already installed. These links are all in the workspace,
not in system directories. The client uses the original Cargo.toml/Cargo.lock;
Cargo was run with `--offline --locked`.

Commands from `client/src-tauri/`, with workspace-local PATH,
PKG_CONFIG_SYSROOT_DIR/PKG_CONFIG_PATH and LD_LIBRARY_PATH:

```
cargo test --offline --workspace --locked --no-fail-fast
cargo build --offline --workspace --locked
```

| Client check | Green | Red | Ignored | Result |
|---|---:|---:|---:|---|
| Rust build | — | — | — | Pass |
| Rust full test suite | 34 | 12 | 5 | All reds are expected assertions |
| Client expected-red gate | — | — | — | Pass; self-tests 26/26 |

The 12 client failures are the existing first-red config surfaces: eight in
`direct_config` and four in `host_config_reload`. They all carry ASSERT markers.
No compile, initialization or linker failure remains. Root and frontend suites
were not repeated in this follow-up because their source and dependency files
were unchanged; the previous recorded results remain root 41 green / 90 red /
1 ignored, frontend 53 green / 0 red.

All final logs and temporary setup files for this follow-up are under
`/workspace/work/pv-resume/`. The Go meta-test's two consumed result files were
also placed in the repository's ignored `target/` directory. All runtime setup
and source verification happened without storage.googleapis.com access or
changes to the managed network configuration. No commit, push or PR was made.

Repository file edits in this follow-up: only `docs/tailcat-pv-report.md`
(this factual follow-up). Hash comparisons confirmed every other tracked or
previously untracked source file, the dependency manifests/locks, acceptance
tests, registry and expected-red list retained their contents from the start of
this follow-up. In particular, the three expected-red deletions shown by git
remain inherited from the initial run; no further entry, including #55, was
added, removed or edited.
