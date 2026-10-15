# Native MCP build and verification

Native MCP is an optional compiled component. Build with `cargo check --locked --features mcp`; ordinary builds exclude the rmcp dependency. The selected SDK uses server-only features. Start the development endpoint with `cargo run --locked --features mcp -- --no-config --debug-server 127.0.0.1:0 --mcp`, then connect to `/mcp` at the printed listener address. The build feature alone does not start a listener. The current tools are `dos_status`, `dos_capture`, `dos_key`, `dos_type`, `dos_mouse` and `dos_operation`.

`dos_mouse` accepts absolute integer picture pixels or guest-relative deltas. Pass the capture's `mouse_picture` to move, click, press a button, or drag; stale pictures reject with `stale_picture` and `capture_again` recovery. A button down requires a lease of at most 30 seconds. Retain its opaque handle to move during the hold, renew its lease, or release it. A finite drag accepts at most 256 points using one coordinate form and at most 5000 ms total delay. Input acceptance can precede delivery: query `dos_operation` until `delivery` is `delivered` before depending on completion. Do not replay input after uncertain acceptance.

Tool annotations identify observations as read-only and guest input as potentially destructive and non-idempotent: keys and mouse events can modify files in mounted directories. Every tool advertises a JSON Schema 2020-12 output contract. Execution failures provide an `error` object with `code`, `message` and `recovery`, alongside a serialized JSON text result. Malformed arguments are protocol errors. The test-only JSON Schema validator checks real SDK results and uses no network or file resolution features. This local endpoint uses no credentials; existing local access guards still apply.

Upstream compiler declarations and release jobs are preserved. The scoped MCP compatibility checks use Rust 1.95.0 and 1.98.1; MCP blocking Clippy uses 1.98.1. No global MSRV or production compiler promise is introduced. This is the operator-approved exception to the team's global floor/pin requirements, rather than complete conformance to those rules.

Run `cargo run --locked --example mcp_check -- foundation` for the available observation/null-backend and verification-harness tests. The local `contracts` command also requires the observation contract, ADR, port/schema sources and approved acquisition manifest before running those tests. `baseline` runs the existing workspace and dynamic suites. `phase 1` also requires local decision/ADR/register records and performs scoped compiler/build checks. Local planning records are intentionally excluded from commits; CI must use the portable foundation target.

`surface` prints a SHA-256 inventory of contract, evidence, build and verification files. Local closure requires that inventory to match `docs/mcp/execution/surface-review.json`, recorded after semantic and evidence review. That local receipt is excluded from commits. CI checks the shared service's transport import boundary through the foundation tests.

`captures --scope shell-input-protocol` verifies actual local acquisition records described by `docs/mcp/execution/acquisition.json`. It checks producer/file hashes, bounded PNG decoding, observed input and save/load outcomes, and reference SDK wire exchanges. The private catalog and raw captures are not bundled or committed. This command verifies acquisition before publication; it does not establish runtime MCP capabilities. Mouse and graphics acquisition targets and source-bound readiness checks are implemented as described below.

`approvals` checks the strict metadata-only publication policy and pinned program manifest. `assets` also verifies actual acquired ZIPs using the private `docs/mcp/execution/programs.json` catalog: `schema_version: 1`, an absolute `raw_root`, and `files` mapping program IDs to relative archive paths. It checks package/source hashes, bounded ZIP directories/decompression and required bundled notices, executables and configuration members. Neither command proves live mouse behavior; package and capture data remain outside commits.

`assets --prepare` creates new isolated guests outside the repository, retains the full package layout and bundled notices, and applies only the recorded application settings. Configuration bytes must match the observed SHA256 pins. It writes a private `guests.json` with mount paths, DOS working directories and launch commands for subsequent live replay; failure removes the partial new directory. Preparation alone does not complete capture readiness.

`challenge unavailable-observation` verifies that its protecting assertion detects the real source mutation in an isolated copy. Registered mouse and keyboard challenges and runtime cases require their actual source-bound evidence; the final agent workflow remains pending. An empty required test suite cannot be masked by passing tooling tests. Diagnostics and subprocess output have finite byte/time limits; failed commands cannot pass the wrapper.

The durable test asset is an [acquisition manifest](../../tests/fixtures/approved/mcp/manifest.json) with pinned Grafx2 and SEAL packages, bundled-source references and application configuration. Acquire programs in temporary test guests and preserve their source/license notices there. Generate screenshots and protocol observations from live runs; keep them outside the repository and release artifacts. Rust-DOS's license, defaults and release configuration remain unchanged. Temporary/planning files remain outside staging and commits.

Grafx2 is GPL-2.0-only and the SEAL source is GPL-2.0-or-later; retain component notices supplied by each package. These are separate DOS test applications. The manifest does not redistribute or link their code into Rust-DOS, so this use requires no change to Rust-DOS's license. See the [GNU GPL FAQ on running programs with an interpreter](https://www.gnu.org/licenses/gpl-faq.html#IfInterpreterIsGPL). Guest packages and captured images remain outside Rust-DOS releases.

With `--mcp`, stderr diagnostics contain Rust-DOS observation context only. The diagnostic subscriber accepts the `rust_dos::mcp::context` target; SDK tracing is excluded because it can include caller headers and complete screenshot responses. Ordinary startup installs no MCP subscriber. `--case privacy` exercises the filtered sink through a live emulator and SDK capture, then enables raw SDK tracing in an isolated test subprocess to prove the same payload scan detects the actual screenshot leak. `--case access` and `--case limits` exercise the native access/resource guards.

`--case capture` builds and retains the executable reported by Cargo, with source snapshots checked before and after the build and acquisition. It checks both negotiated SDK protocols against two changed frames from each actual graphics guest. The private catalog binds the build output, executable, source inputs and observations; stdout artifact records are kept separate from diagnostic output. Image checks include decoded pixel parity, guest instance continuity, dimensions, sequence fields and request order. The same run rejects minimal corruptions of its actual metadata records. `captures --scope native-observations` and `challenge native-observation-metadata` verify the private catalog selected by `docs/mcp/execution/native-captures.json`; they fail if that binding is absent or stale.

`--case capture` verifies native shell capture and runs fresh Grafx2/SEAL journeys for both protocols. It requires the private, verified program catalog described above. Each client captures two changed real guest frames and checks independently decoded pixels against the corresponding paused HTTP captures. Replay and image-order mutations must fail the same oracle. Guest files and observations remain in a new temporary directory outside the repository.

`acquire-shell --raw-root /absolute/path/to/new-capture` replays the shell journey in Rust against the built native `target/debug/rust-dos` executable. The parent directory must exist outside the repository; the output directory must be new. It retains an immutable executable copy, canonical requests, actual responses and a private `catalog.json`, then checks shell output, cross-call holds, strict save/load restoration and editor selection/replacement through the independent acquisition validator. Native process startup, requests and output have bounds; its owned process group is terminated and reaped on completion or failure. This shell-only command does not satisfy the separate reference SDK or graphics readiness requirements. Build the native executable from the intended source snapshot before acquisition.

`acquire-graphics --raw-root /absolute/path/to/new-capture` verifies and prepares the pinned Grafx2/SEAL packages from the private program catalog, then replays their standard/doubled graphics journeys through the same bounded native process owner. It records canonical clicks, relative motion, cross-call and finite drags, delivery journals and actual configured application bytes. The generated private catalog binds program configuration hashes, source revision and the exact native executable. The validator checks held/released positions and decoded window/paint effects; move the cursor away before comparing the painted click location. This command acquires baseline HTTP evidence and does not assert MCP runtime capabilities.

`acquire-mcp-mouse --program grafx2 --raw-root /absolute/path/to/new-capture` runs native MCP mouse journeys against the pinned application; use `--program seal` for the other guest. It records picture-based moves, clicks, held-button motion, renewal, targeted release and finite drags through both supported SDK protocols. `captures --scope native-mouse` checks the private catalog bindings, operation correlation, driver state and visible guest effects. Applications and captures remain outside the repository.

`acquire-protocol --raw-root /absolute/path/to/new-capture` runs the actual rmcp 3.5.1 reference server and client. It records both legacy initialization and modern discovery/tool-list exchanges plus forbidden-Origin rejection, then verifies the wire schema and provenance. The retained executable and source/lockfile hashes prevent later builds from silently relabeling its producer. SDK client and body-recording dependencies are development-only; normal MCP production dependencies remain server-only. The reference probe exposes no guest tools.

`acquire-programs --raw-root /absolute/path/to/new-download` fetches only the approved manifest URLs, with a 30-second request timeout and an 8 MiB archive limit. It checks package/source hashes, versions and bundled notices and writes a private `programs.json` catalog. Use that catalog for subsequent package preparation. Failure removes only the newly created download directory. The downloaded applications remain outside repository and release assets.

`captures --validate` checks approved package/source archives, the metadata-only durable asset directory, and both private acquisition catalogs. The combined shell catalog must include the SDK `protocol_wire` and `protocol_producer` records. Text acquisition also checks finite selection/replacement, click-to-caret and relative motion. This target validates data readiness; phase closure additionally requires the baseline, coverage and remaining documented checks.

`captures --scope mouse` additionally checks private `docs/mcp/execution/mouse.json` observations using the same revision, source and executable hash bindings. It requires standard and doubled graphics dimensions, held/released driver states, and decoded pixels showing SEAL window movement and Grafx2 painting. It also runs the text-mode acquisition checks. Grafx2's DOS port coordinate convention is a baseline limitation; its HTTP painting witness alone does not establish the separate MCP image-coordinate contract. An optional `native_producer` object (`path`, `sha256`) binds observations to a separately retained executable so Cargo rebuilds cannot silently relabel the original producer.

`coverage` verifies the local source-bound LLVM receipt independently of the full gate. The receipt and report remain local and uncommitted. Run the full `phase 2` gate without inherited coverage compiler wrappers: nightly branch-instrumentation flags cannot be passed to the scoped stable compiler checks. Instrumentation of the helper is separate from those compatibility checks.

Native observation catalogs retain each protocol's actual discovery response as well as captures and status. Validation requires exactly `dos_status`, `dos_capture`, `dos_key`, `dos_type`, `dos_mouse` and `dos_operation`, with each input schema matching the independent contract. Observation tools retain empty object inputs; all tools reject additional properties. The metadata challenge alters these acquired schemas and tool names to verify that incompatible discovery is rejected; it does not generate replacement discovery fixtures.

Acquisition-only tooling coverage must preserve the accepted 612/914 branch baseline (approximately 66.96%) across the measured helper scope, with actual nonempty execution of every required helper file. This proportional tooling policy is separate from the 95% critical runtime threshold; a nonzero helper hit count alone cannot close the gate. Native status observations also retain paused HTTP status oracles for program, instruction count and driver mouse state.

`acquire-frontend --raw-root /absolute/path/to/new-capture` requires Linux with `Xvfb` and `xdotool`. It builds and retains the native executable, owns a separate X server, and sends hotkeys only to that server's SDL window. It uses the built-in DOS shell, without generating a guest executable. Physical settings opening must cancel queued native key/text operations; native guest chords must leave the settings window closed. The rewind journey checks actual BIOS clock rollback, native hold invalidation while Alt+F11 remains down, preserved restored BIOS Shift state, and rejection of the old handle. Add `--load` to test actual Ctrl+F1/Ctrl+F2 slot saving/loading, expiry across replacement, and rejection of old up/renew requests. With MCP enabled, restored keyboard state is preserved and old ownership is forgotten before polling; ordinary frontend loading keeps its upstream behavior. Build/source receipts and actual observations remain in the new private directory.

Add `--coverage` to build the frontend with the scoped nightly LLVM instrumentation. This mode exits the emulator gracefully and requires fresh profiles belonging to its PID, retained with source and executable receipts. Behavioral acquisition alone does not establish coverage.

Use `--load --without-mcp` for the ordinary frontend slot-load journey. It verifies that loading retains upstream keyboard-release behavior when the native endpoint is disabled.

With MCP enabled, frontend acquisition also queues a finite drag behind an independent HTTP wait before physical slot loading or rewind. It checks actual clock rollback, cancellation of the pending drag and preservation of the restored button and coordinates after the old wait ends. `captures --scope frontend-mouse` requires these source-bound witnesses alongside the persistent-hold restoration checks.

The local `challenge bypass-local-access` and `challenge bypass-input-limit` commands rerun their real protecting tests and validate source-bound, private seed-kill receipts, exact single-guard mutant bytes and intended failing assertions. `challenge log-request-contents` executes the live filtered/raw-SDK diagnostic sensitivity test. The Phase 3 gate executes these checks; private receipts and mutation logs remain outside commits.

## Attach Codex

The verified client is Codex CLI 0.162.1. Build the native executable with
`cargo +1.98.1 build --locked --features mcp`, then start it with
`target/debug/rust-dos --no-config --mcp --debug-server 127.0.0.1:0 -d /path/to/test/guest`.
Use the printed address plus `/mcp` as the client URL. Loopback admission and Host/Origin
validation remain enabled; the local server requires no credentials.

After setting `dos_mcp_url` to that actual URL, this invocation uses session-only configuration:

```sh
codex exec --ignore-user-config --ephemeral --json --sandbox read-only \
  --skip-git-repo-check --cd /path/to/test/guest \
  -c "mcp_servers.rust_dos.url=\"$dos_mcp_url\"" \
  -c 'mcp_servers.rust_dos.required=true' \
  -c 'mcp_servers.rust_dos.tool_timeout_sec=15' \
  -c 'mcp_servers.rust_dos.default_tools_approval_mode="approve"' \
  'Use only rust_dos tools to observe this DOS guest and report the current screen.'
```

Codex uses its existing CLI-managed authentication; these options do not edit global
configuration. The `approve` setting authorizes guest input tools for this invocation.
The shell sandbox does not make DOS drives read-only: keyboard/mouse input can save,
overwrite or delete guest files. Use an isolated test guest for automated operation.

## Observe, act and hand off

Read `dos_status` and `dos_capture` before acting. For mouse input, set the argument
named `picture` to the returned `mouse_picture` object. Use integer screenshot pixels,
including doubled pictures, or one relative delta pair; do not convert to virtual DOS
driver coordinates. Recapture on `stale_picture`. The image and debugger sequence fields
do not promise an atomic render epoch.

Acceptance and delivery are separate. Keep returned `instance`/`op` and query
`dos_operation` until delivery is `delivered`; unknown acceptance ends the caller's wait
and must not trigger replay. Cancellation or disconnect does not undo delivered input.
Persistent key/button down returns an opaque handle. Observe while held, renew before
its lease expires, qualify mouse motion with that same handle, and explicitly release
with up. Use fresh pictures for motion; up/renew use the handle alone. Finite `drag`
balances its own down/path/up. Incremental dragging uses separate down/move/up calls.

Limits are eight accepted jobs and eight holds, 30-second renewable leases, 4 KiB text,
256 path points and five seconds per balanced gesture. The actor rejects conflicting
foreign or legacy pointer/modifier input before effects. For operator handoff, release
owned holds and finish or explicitly clear pending input. Pause when the selected
workflow permits it; deterministic mutation requires the existing pause policy.
Restore/rewind invalidates old authority while preserving restored device state. Old
handles cannot release input in the restored guest.

Actual Codex editor and Grafx2 journeys exercised cross-observation holds, renewal,
visible dragging and release. Grafx2's DOS port retains a coordinate convention that
can produce painting displaced from requested picture pixels. Local Dungeon Master II
intro/title input worked; the tested `SKULL.EXE` returned to DOS. These observations
do not establish universal game or video compatibility.

## Internal metrics and repeatable client verification

`acquire-client-loop --raw-root /absolute/path/to/new-private-directory` builds and
retains the reported MCP executable, prepares approved application guests, and runs
actual Codex with bounded process/output lifetimes. It records exact version/commands,
real tool responses/images and independent guest observations. Its private catalog
binds current sources, build artifacts and captures. `--case client-loop` verifies the
catalog at `docs/mcp/execution/client-loop.json`; changed inputs require actual reacquisition.
Set `RUST_DOS_CODEX_BIN` only when selecting the executable for that verified client version.

`--case metrics` checks actual successful, rejected, expired and cancelled operations
and live capture capacity. Internal fixed counters count accepted/completed/unsuccessful
input jobs and rejected native requests. Lease-only controls are outside job counts.
Capture occupancy follows the existing retained pipeline slot. Queue residence and
blocking encoding expose sample count, total and maximum nanoseconds. Counters saturate.
The test-only snapshot contains fixed keys; it stores no payloads, IDs or dynamic labels.
There is no public metrics endpoint or exporter, and no atomic cross-counter promise.

The local hardness report at `docs/mcp/atlas.md` records validation boundaries and
remaining limitations. It is retained with private verification evidence and is
excluded from publication; the setup instructions above do not require that report.
