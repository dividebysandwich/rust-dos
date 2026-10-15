//! Fixed verification commands for the native control interface.
#![deny(warnings)]
#![deny(clippy::all)]

use clap::{Parser, Subcommand, ValueEnum};
use std::{
    io,
    process::{Command, ExitCode, Stdio},
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[path = "mcp_check/acquisition.rs"]
mod acquisition;
#[path = "mcp_check/assets.rs"]
mod assets;
#[path = "mcp_check/client.rs"]
mod client;
#[path = "mcp_check/coverage.rs"]
mod coverage;
#[path = "mcp_check/native.rs"]
mod native;
#[path = "mcp_check/protocol.rs"]
mod protocol;
#[path = "mcp_check/replay.rs"]
mod replay;
#[path = "mcp_check/runtime_coverage.rs"]
mod runtime_coverage;

#[derive(Debug, Parser)]
#[command(
    about = "Verify native DOS control",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    target: Option<Target>,
    #[arg(long, value_enum)]
    case: Option<Case>,
}

#[derive(Debug, Subcommand)]
enum Target {
    Foundation,
    Baseline,
    Phase {
        #[arg(value_parser = clap::value_parser!(u8).range(1..=6))]
        number: u8,
    },
    Contracts,
    Captures {
        #[arg(long, value_enum, conflicts_with = "validate")]
        scope: Option<CaptureScope>,
        #[arg(long, required_unless_present = "scope")]
        validate: bool,
    },
    Approvals,
    Assets {
        #[arg(long)]
        prepare: bool,
    },
    AcquireShell {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    AcquireFrontend {
        #[arg(long)]
        raw_root: std::path::PathBuf,
        #[arg(long)]
        load: bool,
        #[arg(long)]
        coverage: bool,
        #[arg(long, requires = "load")]
        without_mcp: bool,
    },
    AcquireCoverage {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    AcquireGraphics {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    AcquireMcpGraphics {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    AcquireMcpMouse {
        #[arg(long)]
        raw_root: std::path::PathBuf,
        #[arg(long, value_parser = ["seal", "grafx2"])]
        program: String,
    },
    AcquireClientLoop {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    MeasureOverhead {
        #[arg(long)]
        raw_root: std::path::PathBuf,
        #[arg(long)]
        baseline: std::path::PathBuf,
    },
    AcquireProtocol {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    AcquirePrograms {
        #[arg(long)]
        raw_root: std::path::PathBuf,
    },
    Surface,
    Coverage,
    RuntimeCoverage,
    Docs,
    Review {
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=6))]
        phase: u8,
    },
    Challenge {
        #[arg(value_enum)]
        defect: Defect,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CaptureScope {
    ShellInputProtocol,
    Mouse,
    NativeObservations,
    NativeMouse,
    FrontendMouse,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Case {
    Discover,
    Capture,
    Key,
    Type,
    Delivery,
    Mouse,
    Access,
    Limits,
    Privacy,
    HttpParity,
    ClientLoop,
    Holds,
    Drag,
    Metrics,
    MouseErrors,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Defect {
    CaptureIntegrity,
    NativeObservationMetadata,
    UnavailableObservation,
    BypassLocalAccess,
    BypassInputLimit,
    LogRequestContents,
    ReplayTimedOutInput,
    ReportQueuedInputDelivered,
    UseVirtualCoordinates,
    ReleaseForeignHold,
    ExecuteExpiredDown,
    OmitDragRelease,
}

#[derive(Debug)]
struct Output {
    bytes: Vec<u8>,
    stdout: Vec<u8>,
    successful: bool,
}

#[cfg(unix)]
struct ProcessTree(i32);

#[cfg(unix)]
impl ProcessTree {
    fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("missing child PID"))?;
        Ok(Self(i32::try_from(pid).map_err(io::Error::other)?))
    }

    fn terminate(&self) {
        // SAFETY: the child was spawned in its own process group. Negative PID
        // targets only that group's processes; no borrowed memory crosses FFI.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: null optional security/name arguments; the returned handle
        // is owned here and closed on every error or Drop path.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let owner = Self(handle);
        // SAFETY: this C structure contains only numeric fields and pointers;
        // all-zero values are valid before specifying the desired limit flag.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: initialized limits of the exact API size, live owned job
        // handle, and a live process handle borrowed from this Child.
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("missing process handle"))?;
        // SAFETY: both handles are live; the child is suspended until membership
        // is established, so it cannot create descendants outside this job.
        if unsafe { AssignProcessToJobObject(handle, process.cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_child(
            child
                .id()
                .ok_or_else(|| io::Error::other("missing child PID"))?,
        )?;
        Ok(owner)
    }

    fn terminate(&self) {
        // SAFETY: this handle remains owned and live until Drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
fn resume_child(pid: u32) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    // SAFETY: snapshots return an owned handle, with INVALID_HANDLE_VALUE on
    // failure. It is transferred to OwnedHandle only after that check.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.cast()) };
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        cntUsage: 0,
        th32ThreadID: 0,
        th32OwnerProcessID: 0,
        tpBasePri: 0,
        tpDeltaPri: 0,
        dwFlags: 0,
    };
    // SAFETY: snapshot is live and entry has the API's required size.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle().cast(), &mut entry) };
    while found != 0 {
        if entry.th32OwnerProcessID == pid {
            // CREATE_SUSPENDED prevents the primary thread from starting or
            // creating more threads. Resume only after assigning the job.
            // SAFETY: the snapshot supplied this thread ID; request only the
            // right needed to resume it, and take ownership after checking.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            let thread = unsafe { OwnedHandle::from_raw_handle(thread.cast()) };
            // SAFETY: the live owned thread handle has THREAD_SUSPEND_RESUME.
            let previous = unsafe { ResumeThread(thread.as_raw_handle().cast()) };
            return match previous {
                1 => Ok(()),
                u32::MAX => Err(io::Error::last_os_error()),
                _ => Err(io::Error::other("unexpected child suspend count")),
            };
        }
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        // SAFETY: snapshot and correctly sized entry remain live.
        found = unsafe { Thread32Next(snapshot.as_raw_handle().cast(), &mut entry) };
    }
    Err(io::Error::other("suspended child thread not found"))
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
        #[cfg(windows)]
        // SAFETY: close the job handle exactly once after its processes end.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

async fn execute(mut command: Command, deadline: Duration, limit: usize) -> io::Result<Output> {
    if deadline.is_zero() || limit == 0 {
        return Err(io::Error::other("invalid process budget"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = command.spawn()?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            child.kill().await?;
            return Err(error);
        }
    };
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr"))?;
    let mut bytes = Vec::new();
    let mut captured_stdout = Vec::new();
    let mut out = [0; 4096];
    let mut err = [0; 4096];
    let (mut out_open, mut err_open) = (true, true);
    let mut status = None;
    let result = tokio::time::timeout(deadline, async {
        while out_open || err_open || status.is_none() {
            let (chunk, from_stdout) = tokio::select! {
                result = stdout.read(&mut out), if out_open => {
                    let n = result?; out_open = n != 0; (&out[..n], true)
                },
                result = stderr.read(&mut err), if err_open => {
                    let n = result?; err_open = n != 0; (&err[..n], false)
                },
                result = child.wait(), if status.is_none() => { status = Some(result?); (&[][..], false) },
            };
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "subprocess output limit",
                ));
            }
            bytes
                .try_reserve_exact(chunk.len())
                .map_err(io::Error::other)?;
            bytes.extend_from_slice(chunk);
            if from_stdout {
                captured_stdout.try_reserve_exact(chunk.len()).map_err(io::Error::other)?;
                captured_stdout.extend_from_slice(chunk);
            }
        }
        Ok(Output {
            bytes,
            stdout: captured_stdout,
            successful: status.is_some_and(|status| status.success()),
        })
    })
    .await
    .unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "subprocess deadline",
        ))
    });
    // Even if the caller's wait ends, explicitly terminate and reap its child.
    // Drop also closes any surviving descendants' inherited output handles.
    tree.terminate();
    if status.is_none() {
        child.start_kill()?;
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .map_err(|_| io::Error::other("child cleanup deadline"))??;
    }
    result
}

async fn run(command: Command, deadline: Duration, limit: usize) -> io::Result<Output> {
    let output = execute(command, deadline, limit).await?;
    if !output.successful {
        return Err(io::Error::other("subprocess exited unsuccessfully"));
    }
    Ok(output)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(error) = std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")) {
        eprintln!("repository unavailable: {error}");
        return ExitCode::FAILURE;
    }
    match dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("verification failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
fn parse(args: &[&str]) -> io::Result<Cli> {
    let cli = Cli::try_parse_from(std::iter::once("mcp_check").chain(args.iter().copied()))
        .map_err(io::Error::other)?;
    if cli.target.is_none() && cli.case.is_none() {
        return Err(io::Error::other("a verification target is required"));
    }
    Ok(cli)
}

fn executed_tests(output: &Output) -> io::Result<usize> {
    let text = std::str::from_utf8(&output.bytes).map_err(io::Error::other)?;
    let mut total = 0usize;
    for line in text.lines() {
        if let Some(result) = line.strip_prefix("test result: ok. ") {
            let (passed, _) = result
                .split_once(" passed;")
                .ok_or_else(|| io::Error::other("malformed test result"))?;
            total = total
                .checked_add(passed.parse::<usize>().map_err(io::Error::other)?)
                .ok_or_else(|| io::Error::other("test count overflow"))?;
        }
    }
    if total == 0 {
        return Err(io::Error::other("no required assertions executed"));
    }
    Ok(total)
}

fn require_file(path: &std::path::Path) -> io::Result<()> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 1024 * 1024 {
        return Err(io::Error::other(
            "required record is empty, non-file or oversized",
        ));
    }
    Ok(())
}

fn observation_contract_documents() -> io::Result<()> {
    for path in [
        "docs/mcp/contracts.md",
        "docs/mcp/adr-002-observations.md",
        "src/debug/control.rs",
        "src/debug/mcp/mod.rs",
        "tests/fixtures/approved/mcp/manifest.json",
    ] {
        require_file(std::path::Path::new(path))?;
    }
    Ok(())
}

fn required_test_groups(output: &Output) -> io::Result<usize> {
    let text = std::str::from_utf8(&output.bytes).map_err(io::Error::other)?;
    for line in text
        .lines()
        .filter(|line| line.starts_with("test result: ok. "))
    {
        executed_tests(&Output {
            bytes: line.as_bytes().to_vec(),
            stdout: Vec::new(),
            successful: output.successful,
        })?;
    }
    executed_tests(output)
}

fn cargo(args: &[&str]) -> Command {
    let mut command;
    if let Some(toolchain) = args.first().and_then(|arg| arg.strip_prefix('+')) {
        command = Command::new("rustup");
        command.args(["run", toolchain, "cargo"]);
        command.args(&args[1..]);
    } else {
        command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command.args(args);
    }
    command
}

async fn cargo_check(args: &[&str], tests: bool) -> io::Result<()> {
    let output = run(cargo(args), Duration::from_secs(1200), 16 * 1024 * 1024).await?;
    if tests {
        let count = if args.contains(&"--test") {
            required_test_groups(&output)?
        } else {
            executed_tests(&output)?
        };
        println!("{count} assertions passed");
    }
    Ok(())
}

async fn foundation() -> io::Result<()> {
    cargo_check(
        &[
            "test",
            "--locked",
            "--test",
            "mcp_foundation",
            "--example",
            "mcp_check",
        ],
        true,
    )
    .await
}

async fn baseline() -> io::Result<()> {
    let mut revision = Command::new("git");
    revision.args([
        "merge-base",
        "--is-ancestor",
        "9ca9396c787a119caafeee88f761f2c5310b9c82",
        "HEAD",
    ]);
    run(revision, Duration::from_secs(10), 8192).await?;
    cargo_check(&["test", "--workspace", "--locked"], true).await?;
    let mut dynamic = cargo(&["test", "--locked"]);
    dynamic.env("RUST_DOS_CORE", "dynamic");
    println!(
        "{} dynamic assertions passed",
        executed_tests(&run(dynamic, Duration::from_secs(1200), 16 * 1024 * 1024).await?)?
    );
    Ok(())
}

struct TemporaryCopy {
    path: std::path::PathBuf,
    cleaned: bool,
}

impl TemporaryCopy {
    fn new() -> io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..32 {
            let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rust-dos-mcp-check-{}-{number}",
                std::process::id()
            ));
            let builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            match builder.create(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        cleaned: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("temporary directory collision limit"))
    }

    fn cleanup(mut self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.path)?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for TemporaryCopy {
    fn drop(&mut self) {
        if !self.cleaned
            && let Err(error) = std::fs::remove_dir_all(&self.path)
        {
            eprintln!("temporary-copy cleanup failed: {:?}", error.kind());
        }
    }
}

fn copy_source(
    source: &std::path::Path,
    destination: &std::path::Path,
    budget: &mut u64,
) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        std::fs::create_dir(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            if entry.file_name() == "target" {
                continue;
            }
            copy_source(&entry.path(), &destination.join(entry.file_name()), budget)?;
        }
    } else if metadata.is_file() {
        *budget = budget
            .checked_add(metadata.len())
            .ok_or_else(|| io::Error::other("copy size overflow"))?;
        if *budget > 512 * 1024 * 1024 || metadata.len() > 64 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "source copy limit",
            ));
        }
        std::fs::copy(source, destination)?;
    } else {
        return Err(io::Error::other(
            "source copy rejects symlinks and special files",
        ));
    }
    Ok(())
}

fn patch_unavailable_observation(path: &std::path::Path) -> io::Result<()> {
    require_file(path)?;
    let source = std::fs::read_to_string(path)?;
    let needle = "Err(ControlError::Unavailable)";
    if source.matches(needle).count() != 2 {
        return Err(io::Error::other(
            "seed source no longer matches the registered defect",
        ));
    }
    std::fs::write(path, source.replace(needle, "Ok(())"))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicySeed {
    schema_version: u8,
    defect: String,
    exit_code: i32,
    sources: std::collections::BTreeMap<String, String>,
    log: std::path::PathBuf,
    log_sha256: String,
    mutant: std::path::PathBuf,
    mutant_sha256: String,
}

async fn policy_seed(defect: Defect) -> io::Result<()> {
    if matches!(defect, Defect::LogRequestContents) {
        return cargo_check(
            &[
                "+1.98.1",
                "test",
                "--locked",
                "--features",
                "mcp",
                "--bin",
                "rust-dos",
                "native_mcp_diagnostics_keep_context_without_guest_payloads",
            ],
            true,
        )
        .await;
    }
    let (name, test, assertion, source, original, replacement) = match defect {
        Defect::BypassLocalAccess => (
            "bypass-local-access",
            "native_mcp_is_opt_in_and_rejects_foreign_host_and_origin",
            "Origin rejected before native service",
            "src/debug/mcp/mod.rs",
            ".enforce_origin_validation();",
            ";",
        ),
        Defect::BypassInputLimit => (
            "bypass-input-limit",
            "native_mcp_rejects_oversized_sdk_metadata",
            "an SDK-generated request above 64 KiB is rejected before actor admission",
            "src/debug/mcp/transport.rs",
            "axum::body::to_bytes(body, 64 * 1024)",
            "axum::body::to_bytes(body, 128 * 1024)",
        ),
        Defect::ReplayTimedOutInput => (
            "replay-timed-out-input",
            "native_mcp_late_actor_timeout_reports_unknown_without_replaying_input",
            "no timed-out request is replayed into the guest",
            "src/debug/control/service.rs",
            "Cmd::NativeInput { input, job },\n                Duration::from_secs(1),\n            )\n            .await\n        {\n            Ok(answer) => answer,\n            Err(SubmissionError::Timeout | SubmissionError::Dropped) => {\n                return Ok(InputOutcome::Unknown { operation });",
            "Cmd::NativeInput { input: input.clone(), job: job.clone() },\n                Duration::from_secs(1),\n            )\n            .await\n        {\n            Ok(answer) => answer,\n            Err(SubmissionError::Timeout | SubmissionError::Dropped) => {\n                let retry_context = self.client.context(None, None);\n                let _ = self.client.submit(&retry_context, Cmd::NativeInput { input, job }, Duration::ZERO).await;\n                return Ok(InputOutcome::Unknown { operation });",
        ),
        Defect::ReportQueuedInputDelivered => (
            "report-queued-input-delivered",
            "native_mcp_deterministic_held_input_survives_client_wait_and_is_queryable",
            "held input is pending until actual delivery",
            "src/debug/control/service.rs",
            "wait: InputWait::Pending,",
            "wait: InputWait::Delivered,",
        ),
        Defect::ReleaseForeignHold => (
            "release-foreign-hold",
            "native_mcp_wrong_and_old_released_handles_preserve_new_same_key_hold",
            "rejection preserves the real owned Ctrl",
            "src/debug/control/input.rs",
            "let message = match self.owned_input.ended_state(handle) {",
            "if !self.owned_input.holds.is_empty() { self.native_retire(cpu, 0, HoldState::Released); }\n            let message = match self.owned_input.ended_state(handle) {",
        ),
        Defect::ExecuteExpiredDown => (
            "execute-expired-down",
            "native_mcp_expired_modifier_borrow_never_delivers_a_late_key",
            "expired borrowed modifier cannot silently deliver the queued key",
            "src/debug/control/input.rs",
            ".is_none_or(|index| self.owned_input.holds[index].deadline <= now)",
            ".is_some_and(|index| self.owned_input.holds[index].deadline <= now)",
        ),
        Defect::UseVirtualCoordinates => (
            "use-virtual-coordinates",
            "native_mcp_mouse_uses_real_captured_text_picture_coordinates",
            "native picture pixels match the real HTTP-backed guest position",
            "src/debug/control/mouse.rs",
            "coords: super::Coords::Screen,",
            "coords: super::Coords::Virtual,",
        ),
        Defect::OmitDragRelease => (
            "omit-drag-release",
            "native_mcp_mouse_stale_partial_drag_releases_only_its_button",
            "stale finite drag cleanup releases its delivered button",
            "src/debug/control/input.rs",
            "&& span.button_delivered",
            "&& false",
        ),
        _ => return Err(io::Error::other("unsupported policy seed")),
    };
    cargo_check(
        &[
            "+1.98.1",
            "test",
            "--locked",
            "--features",
            "mcp",
            "--bin",
            "rust-dos",
            test,
        ],
        true,
    )
    .await?;
    let receipt: PolicySeed = serde_json::from_slice(&acquisition::read_bounded(
        std::path::Path::new(&format!("docs/mcp/execution/{name}.json")),
        64 * 1024,
    )?)
    .map_err(io::Error::other)?;
    acquisition::require(
        receipt.schema_version == 1 && receipt.defect == name && receipt.exit_code == 101,
        "policy seed did not record an intended failing test",
    )?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    acquisition::require(
        receipt.sources.len()
            == runtime_coverage::REQUIRED.len() + runtime_coverage::INVARIANT_SOURCES.len(),
        "policy seed source scope differs",
    )?;
    for source in runtime_coverage::REQUIRED
        .iter()
        .chain(runtime_coverage::INVARIANT_SOURCES)
    {
        let digest = receipt
            .sources
            .get(*source)
            .ok_or_else(|| io::Error::other("policy seed source binding missing"))?;
        acquisition::verify_source(&root.join(source), digest)?;
    }
    for (path, digest) in [
        (&receipt.log, &receipt.log_sha256),
        (&receipt.mutant, &receipt.mutant_sha256),
    ] {
        acquisition::require(
            path.is_absolute() && !path.canonicalize()?.starts_with(&root),
            "policy seed evidence must remain private",
        )?;
        acquisition::verify_source(path, digest)?;
    }
    let baseline = std::fs::read_to_string(root.join(source))?;
    acquisition::require(
        baseline.matches(original).count() == 1,
        "policy guard changed",
    )?;
    let mutant = acquisition::read_bounded(&receipt.mutant, 256 * 1024)?;
    acquisition::require(
        mutant == baseline.replacen(original, replacement, 1).as_bytes(),
        "policy seed changed more than its guard",
    )?;
    let log = acquisition::read_bounded(&receipt.log, 16 * 1024 * 1024)?;
    let log = std::str::from_utf8(&log).map_err(io::Error::other)?;
    acquisition::require(
        log.contains(&format!(
            "test debug::server::http_tests::{test} ... FAILED"
        )) && log.contains(assertion)
            && log.contains("test result: FAILED."),
        "policy seed failed before its protecting assertion",
    )?;
    println!("Verified {name} source mutation killed by its real protecting test");
    Ok(())
}

async fn challenge(defect: Defect) -> io::Result<()> {
    if matches!(
        defect,
        Defect::BypassLocalAccess
            | Defect::BypassInputLimit
            | Defect::LogRequestContents
            | Defect::ReplayTimedOutInput
            | Defect::ReportQueuedInputDelivered
            | Defect::ReleaseForeignHold
            | Defect::ExecuteExpiredDown
            | Defect::UseVirtualCoordinates
            | Defect::OmitDragRelease
    ) {
        return policy_seed(defect).await;
    }
    if !matches!(defect, Defect::UnavailableObservation) {
        require_file(std::path::Path::new(
            "tests/fixtures/approved/mcp/manifest.json",
        ))?;
        return Err(io::Error::other(
            "capability defect has no implemented mutation/evidence binding",
        ));
    }
    foundation().await?;
    let temporary = TemporaryCopy::new()?;
    let result = async {
        let mut budget = 0;
        for name in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-dos.conf.example",
            "build.rs",
            ".cargo",
            "src",
            "crates",
            "tests",
            "examples",
            "assets",
            "packaging",
        ] {
            copy_source(
                std::path::Path::new(name),
                &temporary.path.join(name),
                &mut budget,
            )?;
        }
        patch_unavailable_observation(&temporary.path.join("src/debug/control.rs"))?;
        let mut command = cargo(&["test", "--locked", "--test", "mcp_foundation"]);
        command.current_dir(&temporary.path);
        command.env("CARGO_TARGET_DIR", temporary.path.join("target"));
        let output = execute(command, Duration::from_secs(1200), 16 * 1024 * 1024).await?;
        let text = std::str::from_utf8(&output.bytes).map_err(io::Error::other)?;
        if output.successful
            || !text.contains("test null_observations_never_invent_status_or_pixels ... FAILED")
        {
            eprintln!("{text}");
            return Err(io::Error::other(
                "seed survived or failed before its required assertion",
            ));
        }
        println!("unavailable-observation defect killed by its actual assertion");
        Ok(())
    }
    .await;
    temporary.cleanup()?;
    result
}

fn fact_surface(path: &str) -> bool {
    matches!(
        path,
        "Cargo.toml"
            | "Cargo.lock"
            | ".gitignore"
            | "src/lib.rs"
            | "src/main.rs"
            | "src/args.rs"
            | "src/startup.rs"
            | "src/autoinput.rs"
            | "src/mouse_capture.rs"
            | "examples/mcp_check.rs"
            | "tests/debug_journal_tests.rs"
            | "docs/fact-ledger.md"
            | "docs/evidence-index.md"
            | "README.md"
            | "docs/llm-debugging.md"
    ) || path.starts_with("tests/mcp_")
        || path.starts_with("src/debug/")
        || path.starts_with("src/capture/")
        || path.starts_with("tests/fixtures/approved/mcp/")
        || path == "tests/fixtures/mcp/mouse.schema.json"
        || path.starts_with(".cargo/")
        || path.starts_with(".github/workflows/")
        || path.starts_with("schemas/")
        || path.starts_with("examples/mcp_check/")
        || (path.starts_with("docs/mcp/")
            && !path.starts_with("docs/mcp/captures/")
            && !path.starts_with("docs/mcp/execution/"))
}

async fn surface_inventory() -> io::Result<std::collections::BTreeMap<String, String>> {
    use sha2::{Digest, Sha256};
    let mut command = Command::new("git");
    command.args([
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ]);
    let output = run(command, Duration::from_secs(10), 1024 * 1024).await?;
    let mut paths = std::collections::BTreeSet::new();
    for raw in output
        .bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(raw).map_err(io::Error::other)?;
        if fact_surface(path) {
            paths.insert(path.to_owned());
        }
    }
    // Local review records are intentionally ignored by Git. Enumerate those
    // contract documents explicitly, while excluding raw captures and the
    // review receipt itself to avoid a self-referential fingerprint.
    let mut ignored = Command::new("git");
    ignored.args([
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--",
        "docs/mcp",
        "docs/fact-ledger.md",
        "docs/evidence-index.md",
    ]);
    let output = run(ignored, Duration::from_secs(10), 1024 * 1024).await?;
    for raw in output
        .bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(raw).map_err(io::Error::other)?;
        if fact_surface(path) {
            paths.insert(path.to_owned());
        }
    }
    let mut inventory = std::collections::BTreeMap::new();
    for path in paths {
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
            return Err(io::Error::other("invalid fact surface file"));
        }
        let mut file = std::fs::File::open(&path)?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 8192];
        loop {
            let count = std::io::Read::read(&mut file, &mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        let digest = hash.finalize();
        let fingerprint = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        inventory.insert(path, fingerprint);
    }
    Ok(inventory)
}

async fn surface() -> io::Result<()> {
    let inventory = surface_inventory().await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&inventory).map_err(io::Error::other)?
    );
    let path = std::path::Path::new("docs/mcp/execution/surface-review.json");
    require_file(path)?;
    let reviewed: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
    if reviewed != inventory {
        return Err(io::Error::other(
            "fact surface changed: semantic/evidence review required",
        ));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PhaseReview {
    schema_version: u8,
    phase: u8,
    verdict: String,
    reviewer: String,
    statement: std::path::PathBuf,
    statement_sha256: String,
    sources: std::collections::BTreeMap<String, String>,
    evidence: std::collections::BTreeMap<String, String>,
}

async fn review_phase(phase: u8) -> io::Result<()> {
    acquisition::require(
        matches!(phase, 3..=6),
        "unsupported independent review phase",
    )?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    let review: PhaseReview = serde_json::from_slice(&acquisition::read_bounded(
        &root.join(format!("docs/mcp/execution/phase-{phase}-review.json")),
        64 * 1024,
    )?)
    .map_err(io::Error::other)?;
    acquisition::require(
        review.schema_version == 1
            && review.phase == phase
            && review.verdict == "PASS"
            && !review.reviewer.is_empty()
            && review.reviewer != "/root",
        "an unconditional independent review is required",
    )?;
    acquisition::require(
        review.statement.is_absolute() && !review.statement.starts_with(&root),
        "independent review statement must remain private",
    )?;
    acquisition::verify_source(&review.statement, &review.statement_sha256)?;
    verify_review_inventory(&review.sources, &surface_inventory().await?)?;
    let mut required_evidence = vec![
        "acquisition.json",
        "mouse.json",
        "native-captures.json",
        "runtime-coverage.json",
        "tooling-coverage.json",
        "bypass-local-access.json",
        "bypass-input-limit.json",
    ];
    if phase >= 4 {
        required_evidence.extend([
            "replay-timed-out-input.json",
            "report-queued-input-delivered.json",
            "release-foreign-hold.json",
            "execute-expired-down.json",
        ]);
    }
    if phase >= 5 {
        required_evidence.extend([
            "native-mouse.json",
            "frontend-mouse.json",
            "use-virtual-coordinates.json",
            "omit-drag-release.json",
        ]);
    }
    if phase == 6 {
        required_evidence.extend(["client-loop.json", "final-hardening.json"]);
    }
    for evidence in required_evidence {
        let key = format!("docs/mcp/execution/{evidence}");
        let digest = review
            .evidence
            .get(&key)
            .ok_or_else(|| io::Error::other("independent review evidence binding missing"))?;
        acquisition::verify_source(&root.join(&key), digest)?;
    }
    println!("Verified independent Phase {phase} PASS bound to current source and evidence");
    Ok(())
}

fn verify_review_inventory(
    reviewed: &std::collections::BTreeMap<String, String>,
    actual: &std::collections::BTreeMap<String, String>,
) -> io::Result<()> {
    acquisition::require(
        reviewed == actual && !actual.is_empty(),
        "independent review source surface is stale",
    )
}

fn verify_docs(root: &std::path::Path) -> io::Result<()> {
    for (path, markers) in [
        ("README.md", &["docs/mcp/README.md"][..]),
        (
            "docs/llm-debugging.md",
            &["mcp/README.md", "--mcp", "mounted guest drives"][..],
        ),
        (
            "docs/mcp/README.md",
            &[
                "0.162.1",
                "--ignore-user-config",
                "--ephemeral",
                "required=true",
                "tool_timeout_sec=15",
                "picture",
                "mouse_picture",
                "dos_operation",
                "stale_picture",
                "30-second",
                "256",
                "five seconds",
                "nanoseconds",
                "approve",
                "atlas.md",
            ][..],
        ),
        (
            "docs/mcp/atlas.md",
            &[
                "Edge cases",
                "Tool/service failure",
                "Concurrency",
                "Security",
                "Verification",
                "Limits",
            ][..],
        ),
    ] {
        let bytes = acquisition::read_bounded(&root.join(path), 256 * 1024)?;
        let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
        acquisition::require(
            markers.iter().all(|marker| text.contains(marker)),
            "required operating documentation is incomplete",
        )?;
    }
    acquisition::require(
        root.join("docs/mcp/README.md").is_file() && root.join("docs/mcp/atlas.md").is_file(),
        "native documentation link target missing",
    )?;
    println!("Verified native setup, operating limits and hardness documentation");
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HardeningCheck {
    command: Vec<String>,
    exit_code: i32,
    log: std::path::PathBuf,
    log_sha256: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalHardening {
    schema_version: u8,
    sources: std::collections::BTreeMap<String, String>,
    checks: std::collections::BTreeMap<String, HardeningCheck>,
}
async fn verify_final_hardening() -> io::Result<()> {
    let receipt: FinalHardening = serde_json::from_slice(&acquisition::read_bounded(
        std::path::Path::new("docs/mcp/execution/final-hardening.json"),
        256 * 1024,
    )?)
    .map_err(io::Error::other)?;
    acquisition::require(
        receipt.schema_version == 1 && receipt.checks.len() == 5,
        "final hardening scope mismatch",
    )?;
    verify_review_inventory(&receipt.sources, &surface_inventory().await?)?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    for name in [
        "metrics-overhead",
        "dependency-advisories",
        "dependency-licenses",
        "feature-compatibility",
        "clean-snapshot",
    ] {
        let check = receipt
            .checks
            .get(name)
            .ok_or_else(|| io::Error::other("required final check missing"))?;
        acquisition::require(
            check.exit_code == 0
                && !check.command.is_empty()
                && check.command.len() <= 64
                && check.log.is_absolute()
                && !check.log.starts_with(&root),
            "final check failed or has no private command evidence",
        )?;
        acquisition::verify_source(&check.log, &check.log_sha256)?;
        acquisition::read_bounded(&check.log, 32 * 1024 * 1024)?;
    }
    println!("Verified source-bound final measurements, dependencies and clean-snapshot checks");
    Ok(())
}

async fn data_readiness() -> io::Result<()> {
    let manifest = std::path::Path::new("tests/fixtures/approved/mcp/manifest.json");
    assets::approvals(manifest)?;
    assets::verify_packages(
        manifest,
        std::path::Path::new("docs/mcp/execution/programs.json"),
    )?;
    acquisition::verify_public_assets(std::path::Path::new("tests/fixtures/approved/mcp"))?;
    let mut revision = Command::new("git");
    revision.args(["rev-parse", "HEAD"]);
    let output = run(revision, Duration::from_secs(10), 8192).await?;
    let revision = std::str::from_utf8(&output.bytes)
        .map_err(io::Error::other)?
        .trim();
    acquisition::verify(
        std::path::Path::new("docs/mcp/execution/acquisition.json"),
        revision,
    )?;
    acquisition::verify_graphics(
        std::path::Path::new("docs/mcp/execution/mouse.json"),
        revision,
    )
}

async fn dispatch(cli: Cli) -> io::Result<()> {
    if matches!(
        cli.case,
        Some(
            Case::Access
                | Case::Limits
                | Case::Privacy
                | Case::Key
                | Case::Type
                | Case::Holds
                | Case::Delivery
                | Case::Mouse
                | Case::Drag
                | Case::MouseErrors
                | Case::Metrics
        )
    ) {
        let tests: &[Option<&str>] = match cli.case {
            Some(Case::Metrics) => &[
                Some("native_mcp_metrics_observe_actual_outcomes_and_bounded_labels"),
                Some("native_mcp_capture_admission_precedes_a_second_frame_request"),
            ],
            Some(Case::Mouse) => &[
                Some("native_mcp_mouse_"),
                Some("native_mcp_output_schemas_validate_real_guest_results_and_errors"),
            ],
            Some(Case::Drag) => &[
                Some("native_mcp_mouse_stale_partial_drag_releases_only_its_button"),
                Some("native_mcp_mouse_maximum_path_admission_and_storage_are_bounded"),
                Some("native_mcp_mouse_maximum_path_plays_once_after_backlog_and_disconnect"),
                Some("native_mcp_mouse_button_claim_survives_disconnect_and_qualifies_motion"),
            ],
            Some(Case::MouseErrors) => &[
                Some("native_mcp_mouse_invalid_requests_have_no_queued_or_guest_prefix"),
                Some("native_mcp_mouse_stale_picture_reports_recapture_without_queue_effects"),
                Some("native_mcp_mouse_rejects_null_path_fields_before_acceptance"),
                Some("native_mcp_mouse_rejects_motion_during_an_http_button_hold"),
                Some("native_mcp_mouse_hold_rejects_unowned_http_pointer_events"),
                Some("native_mcp_mouse_vbe_mode_identity_invalidates_same_geometry_picture"),
                Some("native_mcp_mouse_running_deterministic_input_respects_existing_pause_policy"),
            ],
            Some(Case::Delivery) => &[
                Some("native_mcp_operation_distinguishes_delivered_down_from_retained_hold"),
                Some(
                    "native_mcp_saved_keyboard_restore_invalidates_old_handle_without_break_codes",
                ),
                Some("native_mcp_expired_modifier_borrow_never_delivers_a_late_key"),
                Some("native_mcp_admin_clear_invalidates_queued_hold_and_releases_job_capacity"),
                Some("native_mcp_admin_clear_preserves_an_already_delivered_down"),
                Some("native_mcp_deterministic_held_input_survives_client_wait_and_is_queryable"),
                Some("native_mcp_shutdown_releases_owned_keyboard_before_immutable_shutdown"),
                Some("native_mcp_running_deterministic_input_respects_existing_pause_policy"),
                Some("native_mcp_late_actor_timeout_reports_unknown_without_replaying_input"),
                Some("native_mcp_settings_handoff_resets_owned_jobs_and_handles"),
                Some("native_mcp_frontend_restore_hook_detaches_old_keys_without_break_codes"),
                Some("native_mcp_restore_detaches_an_active_native_wait_without_break_codes"),
                Some("native_mcp_input_respects_shared_pending_operation_capacity"),
                Some("native_mcp_full_job_capacity_still_allows_targeted_hold_control"),
                Some("native_mcp_evicted_operation_does_not_revoke_a_live_hold"),
                Some("native_mcp_expiry_after_all_downs_preserves_delivered_operation"),
                Some("native_mcp_partial_down_expiry_releases_only_delivered_modifiers"),
                Some("native_mcp_ended_hold_errors_distinguish_expired_and_released"),
                Some("native_mcp_invalid_hold_and_operation_requests_preserve_live_authority"),
                Some("native_mcp_live_hold_capacity_is_separate_from_delivered_job_capacity"),
                Some("native_mcp_ended_hold_eviction_cannot_release_a_new_owner"),
                Some(
                    "native_input_port_rejects_noncanonical_keys_and_unbounded_delay_before_effects",
                ),
                Some("native_mcp_guest_shortcuts_reach_bios_without_frontend_hotkeys"),
                Some("native_mcp_target_transition_cancels_pending_input_before_dispatch"),
                Some("native_mcp_target_transition_preserves_an_already_delivered_down"),
            ],
            Some(Case::Privacy) => &[Some(
                "native_mcp_diagnostics_keep_context_without_guest_payloads",
            )],
            Some(Case::Key) => &[
                Some("native_mcp_enter_delivers_owned_operation_to_real_shell"),
                Some("native_mcp_selected_modifiers_reach_real_bios_and_release"),
                Some("native_mcp_canonical_dir_and_enter_show_actual_guest_files"),
                Some("native_mcp_invalid_keyboard_requests_allocate_no_actor_work"),
            ],
            Some(Case::Type) => &[
                Some("native_mcp_typing_delivers_real_shell_output_and_journal"),
                Some("native_mcp_canonical_dir_and_enter_show_actual_guest_files"),
                Some("native_mcp_invalid_keyboard_requests_allocate_no_actor_work"),
            ],
            Some(Case::Holds) => &[
                Some("native_mcp_ctrl_hold_survives_protocol_disconnect_and_renews_then_releases"),
                Some("native_mcp_wrong_and_old_released_handles_preserve_new_same_key_hold"),
                Some("native_mcp_foreign_plain_input_conflicts_with_owned_ctrl_and_shift"),
                Some(
                    "native_mcp_queued_holds_and_legacy_modifier_reservations_conflict_before_effects",
                ),
                Some("native_mcp_modifier_sides_cannot_share_or_toggle_an_owned_effect"),
                Some("native_mcp_projects_remaining_legacy_modifier_sides_after_a_release"),
                Some("native_mcp_queued_legacy_modifier_sides_keep_exact_borrowing"),
                Some("native_mcp_typing_cannot_release_a_retained_character_key"),
                Some("native_mcp_mouse_button_claim_survives_disconnect_and_qualifies_motion"),
                Some("native_mcp_mouse_and_key_handles_remain_device_specific_after_release"),
                Some("native_mcp_mouse_expiry_preserves_an_independent_modifier_hold"),
                Some(
                    "native_mcp_mouse_full_job_capacity_allows_up_and_renew_without_clearing_keys",
                ),
                Some("native_mcp_mouse_delivered_hold_survives_mode_change_and_requires_recapture"),
                Some("native_mcp_mouse_restore_detaches_old_authority_without_button_up"),
            ],
            Some(Case::Limits) => &[None],
            _ => &[Some("native_")],
        };
        for test in tests {
            let mut arguments = vec![
                "+1.98.1",
                "test",
                "--locked",
                "--features",
                "mcp",
                "--bin",
                "rust-dos",
            ];
            if let Some(test) = test {
                arguments.push(test);
            }
            cargo_check(&arguments, true).await?;
        }
        return Ok(());
    }
    if matches!(cli.case, Some(Case::ClientLoop)) {
        let mut revision = Command::new("git");
        revision.args(["rev-parse", "HEAD"]);
        let output = run(revision, Duration::from_secs(10), 8192).await?;
        return client::verify_catalog(
            std::path::Path::new("docs/mcp/execution/client-loop.json"),
            std::str::from_utf8(&output.bytes)
                .map_err(io::Error::other)?
                .trim(),
        );
    }
    if matches!(cli.case, Some(Case::Discover | Case::Capture)) {
        assets::approvals(std::path::Path::new(
            "tests/fixtures/approved/mcp/manifest.json",
        ))?;
        cargo_check(
            &[
                "+1.98.1",
                "test",
                "--locked",
                "--features",
                "mcp",
                "--bin",
                "rust-dos",
                "native_mcp_discovery_and_shell_capture",
            ],
            true,
        )
        .await?;
        if matches!(cli.case, Some(Case::Capture)) {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "rust-dos-mcp-capture-{}-{stamp}",
                std::process::id()
            ));
            replay::native_graphics(&root).await?;
        }
        return Ok(());
    }
    if matches!(cli.case, Some(Case::HttpParity)) {
        assets::approvals(std::path::Path::new(
            "tests/fixtures/approved/mcp/manifest.json",
        ))?;
        return cargo_check(
            &[
                "+1.98.1",
                "test",
                "--locked",
                "--bin",
                "rust-dos",
                "native_observation_http_parity",
            ],
            true,
        )
        .await;
    }
    if cli.case.is_some() {
        require_file(std::path::Path::new(
            "tests/fixtures/approved/mcp/manifest.json",
        ))?;
        return Err(io::Error::other("evidence case has no implemented binding"));
    }
    match cli
        .target
        .ok_or_else(|| io::Error::other("a verification target is required"))?
    {
        Target::AcquireCoverage { raw_root } => replay::coverage(&raw_root).await,
        Target::Foundation => foundation().await,
        Target::Contracts => {
            observation_contract_documents()?;
            foundation().await
        }
        Target::Baseline => baseline().await,
        Target::Phase {
            number: phase @ (2..=6),
        } => {
            if phase >= 3 {
                observation_contract_documents()?;
            }
            data_readiness().await?;
            surface().await?;
            baseline().await?;
            for toolchain in ["+1.95.0", "+1.98.1"] {
                cargo_check(
                    &[
                        toolchain,
                        "test",
                        "--locked",
                        "--features",
                        "mcp",
                        "--example",
                        "mcp_check",
                        "--test",
                        "mcp_foundation",
                    ],
                    true,
                )
                .await?;
                cargo_check(
                    &[
                        toolchain,
                        "check",
                        "--locked",
                        "--no-default-features",
                        "--lib",
                    ],
                    false,
                )
                .await?;
                cargo_check(
                    &[toolchain, "check", "--locked", "--features", "mcp"],
                    false,
                )
                .await?;
            }
            let mut format = Command::new("rustup");
            format.args([
                "run",
                "1.98.1",
                "rustfmt",
                "--edition",
                "2024",
                "--check",
                "examples/mcp_check.rs",
                "examples/mcp_check/acquisition.rs",
                "examples/mcp_check/assets.rs",
                "examples/mcp_check/replay.rs",
                "examples/mcp_check/protocol.rs",
                "examples/mcp_check/coverage.rs",
                "examples/mcp_check/runtime_coverage.rs",
                "examples/mcp_check/native.rs",
                "examples/mcp_check/native/catalog.rs",
                "src/debug/control.rs",
                "src/debug/control/service.rs",
                "src/debug/mcp/mod.rs",
                "src/debug/mcp/transport.rs",
                "src/debug/mcp/sessions.rs",
                "src/debug/mcp/diagnostics.rs",
                "tests/mcp_foundation.rs",
            ]);
            if phase >= 4 {
                format.args([
                    "src/debug/control/input.rs",
                    "src/debug/mcp/keyboard.rs",
                    "examples/mcp_check/replay/frontend.rs",
                ]);
            }
            if phase >= 5 {
                format.args([
                    "src/debug/control/mouse.rs",
                    "src/debug/mcp/mouse.rs",
                    "src/debug/mcp/schemas.rs",
                ]);
            }
            if phase == 6 {
                format.args([
                    "examples/mcp_check/client.rs",
                    "src/debug/control/metrics.rs",
                    "src/debug/mcp/metrics.rs",
                    "tests/mcp_metrics.rs",
                ]);
            }
            run(format, Duration::from_secs(60), 1024 * 1024).await?;
            cargo_check(
                &[
                    "+1.98.1",
                    "clippy",
                    "--locked",
                    "--no-deps",
                    "--example",
                    "mcp_check",
                    "--test",
                    "mcp_foundation",
                ],
                false,
            )
            .await?;
            coverage::verify(std::path::Path::new(
                "docs/mcp/execution/tooling-coverage.json",
            ))?;
            if phase >= 3 {
                cargo_check(
                    &[
                        "+1.98.1",
                        "test",
                        "--locked",
                        "--features",
                        "mcp",
                        "--bin",
                        "rust-dos",
                    ],
                    true,
                )
                .await?;
                cargo_check(
                    &[
                        "+1.98.1",
                        "clippy",
                        "--locked",
                        "--no-deps",
                        "--features",
                        "mcp",
                        "--bin",
                        "rust-dos",
                        "--example",
                        "mcp_check",
                        "--test",
                        "mcp_foundation",
                    ],
                    false,
                )
                .await?;
                runtime_coverage::verify(std::path::Path::new(
                    "docs/mcp/execution/runtime-coverage.json",
                ))?;
                for defect in [
                    Defect::BypassLocalAccess,
                    Defect::BypassInputLimit,
                    Defect::LogRequestContents,
                ] {
                    policy_seed(defect).await?;
                }
                if phase >= 4 {
                    for defect in [
                        Defect::ReplayTimedOutInput,
                        Defect::ReportQueuedInputDelivered,
                        Defect::ReleaseForeignHold,
                        Defect::ExecuteExpiredDown,
                    ] {
                        policy_seed(defect).await?;
                    }
                    for case in ["key", "type", "holds", "delivery"] {
                        cargo_check(
                            &[
                                "+1.98.1",
                                "run",
                                "--locked",
                                "--example",
                                "mcp_check",
                                "--",
                                "--case",
                                case,
                            ],
                            false,
                        )
                        .await?;
                    }
                }
                if phase >= 5 {
                    for defect in [Defect::UseVirtualCoordinates, Defect::OmitDragRelease] {
                        policy_seed(defect).await?;
                    }
                    for case in ["mouse", "drag", "mouse-errors"] {
                        cargo_check(
                            &[
                                "+1.98.1",
                                "run",
                                "--locked",
                                "--example",
                                "mcp_check",
                                "--",
                                "--case",
                                case,
                            ],
                            false,
                        )
                        .await?;
                    }
                }
                let mut revision = Command::new("git");
                revision.args(["rev-parse", "HEAD"]);
                let output = run(revision, Duration::from_secs(10), 8192).await?;
                let revision = std::str::from_utf8(&output.bytes)
                    .map_err(io::Error::other)?
                    .trim();
                if phase >= 5 {
                    native::verify_mouse_evidence(
                        std::path::Path::new("docs/mcp/execution/native-mouse.json"),
                        revision,
                    )?;
                    native::verify_frontend_mouse_evidence(
                        std::path::Path::new("docs/mcp/execution/frontend-mouse.json"),
                        revision,
                    )?;
                }
                native::challenge_metadata(
                    std::path::Path::new("docs/mcp/execution/native-captures.json"),
                    revision,
                )?;
                if phase == 6 {
                    verify_docs(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))?;
                    client::verify_catalog(
                        std::path::Path::new("docs/mcp/execution/client-loop.json"),
                        revision,
                    )?;
                    for features in ["sdl", "mcp"] {
                        cargo_check(
                            &[
                                "+1.98.1",
                                "check",
                                "--locked",
                                "--no-default-features",
                                "--features",
                                features,
                                "--bin",
                                "rust-dos",
                            ],
                            false,
                        )
                        .await?;
                    }
                    cargo_check(
                        &[
                            "+1.98.1",
                            "run",
                            "--locked",
                            "--example",
                            "mcp_check",
                            "--",
                            "--case",
                            "metrics",
                        ],
                        false,
                    )
                    .await?;
                    verify_final_hardening().await?;
                }
                review_phase(phase).await?;
            }
            Ok(())
        }
        Target::Phase { number: 1 } => {
            for path in [
                "docs/mcp/decisions.md",
                "docs/mcp/adr-001.md",
                "docs/mcp/gotcha.md",
                "docs/fact-ledger.md",
                "docs/evidence-index.md",
            ] {
                require_file(std::path::Path::new(path))?;
            }
            foundation().await?;
            surface().await?;
            baseline().await?;
            let mut format = Command::new("rustup");
            format.args([
                "run",
                "1.98.1",
                "rustfmt",
                "--edition",
                "2024",
                "--check",
                "examples/mcp_check.rs",
                "src/debug/control.rs",
                "tests/mcp_foundation.rs",
            ]);
            run(format, Duration::from_secs(60), 1024 * 1024).await?;
            for toolchain in ["+1.95.0", "+1.98.1"] {
                cargo_check(
                    &[
                        toolchain,
                        "test",
                        "--locked",
                        "--test",
                        "mcp_foundation",
                        "--example",
                        "mcp_check",
                    ],
                    true,
                )
                .await?;
                cargo_check(
                    &[
                        toolchain,
                        "check",
                        "--locked",
                        "--no-default-features",
                        "--lib",
                    ],
                    false,
                )
                .await?;
                cargo_check(
                    &[toolchain, "check", "--locked", "--features", "mcp"],
                    false,
                )
                .await?;
            }
            cargo_check(
                &[
                    "+1.98.1",
                    "clippy",
                    "--locked",
                    "--no-deps",
                    "--example",
                    "mcp_check",
                    "--test",
                    "mcp_foundation",
                ],
                false,
            )
            .await
        }
        Target::Approvals => assets::approvals(std::path::Path::new(
            "tests/fixtures/approved/mcp/manifest.json",
        )),
        Target::Assets { prepare } => {
            let manifest = std::path::Path::new("tests/fixtures/approved/mcp/manifest.json");
            let catalog = std::path::Path::new("docs/mcp/execution/programs.json");
            if prepare {
                assets::prepare_packages(manifest, catalog).map(|_| ())
            } else {
                assets::verify_packages(manifest, catalog)
            }
        }
        Target::AcquireShell { raw_root } => replay::shell(&raw_root).await,
        Target::AcquireFrontend {
            raw_root,
            load,
            coverage,
            without_mcp,
        } => replay::frontend::acquire(&raw_root, load, coverage, without_mcp).await,
        Target::AcquireGraphics { raw_root } => replay::graphics(&raw_root).await,
        Target::AcquireMcpGraphics { raw_root } => replay::native_graphics(&raw_root).await,
        Target::AcquireMcpMouse { raw_root, program } => {
            replay::native_mouse(&raw_root, &program).await
        }
        Target::AcquireClientLoop { raw_root } => client::acquire(&raw_root).await,
        Target::MeasureOverhead { raw_root, baseline } => {
            replay::measure_overhead(&raw_root, &baseline).await
        }
        Target::AcquireProtocol { raw_root } => protocol::acquire(&raw_root).await,
        Target::AcquirePrograms { raw_root } => assets::download_packages(
            std::path::Path::new("tests/fixtures/approved/mcp/manifest.json"),
            &raw_root,
        ),
        Target::Phase { .. } => {
            require_file(std::path::Path::new(
                "tests/fixtures/approved/mcp/manifest.json",
            ))?;
            Err(io::Error::other("runtime evidence gate is not implemented"))
        }
        Target::Captures { scope, validate } => {
            if validate {
                return data_readiness().await;
            }
            if matches!(
                scope,
                Some(
                    CaptureScope::NativeObservations
                        | CaptureScope::NativeMouse
                        | CaptureScope::FrontendMouse
                )
            ) {
                let mut revision = Command::new("git");
                revision.args(["rev-parse", "HEAD"]);
                let output = run(revision, Duration::from_secs(10), 8192).await?;
                let revision = std::str::from_utf8(&output.bytes)
                    .map_err(io::Error::other)?
                    .trim();
                if matches!(scope, Some(CaptureScope::NativeMouse)) {
                    return native::verify_mouse_evidence(
                        std::path::Path::new("docs/mcp/execution/native-mouse.json"),
                        revision,
                    );
                }
                if matches!(scope, Some(CaptureScope::FrontendMouse)) {
                    return native::verify_frontend_mouse_evidence(
                        std::path::Path::new("docs/mcp/execution/frontend-mouse.json"),
                        revision,
                    );
                }
                return native::verify_catalog(
                    std::path::Path::new("docs/mcp/execution/native-captures.json"),
                    revision,
                );
            }
            if matches!(
                scope,
                Some(CaptureScope::ShellInputProtocol | CaptureScope::Mouse)
            ) {
                let mut revision = Command::new("git");
                revision.args(["rev-parse", "HEAD"]);
                let output = run(revision, Duration::from_secs(10), 8192).await?;
                let revision = std::str::from_utf8(&output.bytes)
                    .map_err(io::Error::other)?
                    .trim();
                acquisition::verify(
                    std::path::Path::new("docs/mcp/execution/acquisition.json"),
                    revision,
                )?;
                if matches!(scope, Some(CaptureScope::Mouse)) {
                    acquisition::verify_graphics(
                        std::path::Path::new("docs/mcp/execution/mouse.json"),
                        revision,
                    )?;
                }
                return Ok(());
            }
            Err(io::Error::other(
                "capture acquisition/validation not implemented",
            ))
        }
        Target::Review {
            phase: phase @ (3..=6),
        } => review_phase(phase).await,
        Target::Review { phase } => Err(io::Error::other(format!(
            "review binding for phase {phase} not implemented"
        ))),
        Target::Challenge {
            defect: Defect::CaptureIntegrity,
        } => {
            let mut revision = Command::new("git");
            revision.args(["rev-parse", "HEAD"]);
            let output = run(revision, Duration::from_secs(10), 8192).await?;
            let revision = std::str::from_utf8(&output.bytes)
                .map_err(io::Error::other)?
                .trim();
            acquisition::challenge(
                std::path::Path::new("docs/mcp/execution/acquisition.json"),
                revision,
                false,
            )?;
            acquisition::challenge(
                std::path::Path::new("docs/mcp/execution/mouse.json"),
                revision,
                true,
            )
        }
        Target::Challenge {
            defect: Defect::NativeObservationMetadata,
        } => {
            let mut revision = Command::new("git");
            revision.args(["rev-parse", "HEAD"]);
            let output = run(revision, Duration::from_secs(10), 8192).await?;
            let revision = std::str::from_utf8(&output.bytes)
                .map_err(io::Error::other)?
                .trim();
            native::challenge_metadata(
                std::path::Path::new("docs/mcp/execution/native-captures.json"),
                revision,
            )
        }
        Target::Challenge { defect } => challenge(defect).await,
        Target::Surface => surface().await,
        Target::Coverage => coverage::verify(std::path::Path::new(
            "docs/mcp/execution/tooling-coverage.json",
        )),
        Target::RuntimeCoverage => runtime_coverage::verify(std::path::Path::new(
            "docs/mcp/execution/runtime-coverage.json",
        )),
        Target::Docs => verify_docs(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pinned_program_acquisition_requires_an_explicit_private_output() {
        assert!(
            Cli::try_parse_from([
                "mcp_check",
                "acquire-programs",
                "--raw-root",
                "/tmp/new-programs"
            ])
            .is_ok()
        );
    }

    #[test]
    fn reference_sdk_acquisition_has_an_explicit_private_output() {
        assert!(
            Cli::try_parse_from([
                "mcp_check",
                "acquire-protocol",
                "--raw-root",
                "/tmp/new-wire"
            ])
            .is_ok()
        );
    }

    #[test]
    fn approved_graphics_replay_is_an_explicit_command() {
        assert!(
            Cli::try_parse_from([
                "mcp_check",
                "acquire-graphics",
                "--raw-root",
                "/tmp/new-capture"
            ])
            .is_ok()
        );
    }

    fn worker(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "tests::child_worker", "--ignored", "--nocapture"]);
        command.env("RUST_DOS_CHECK_WORKER", mode);
        command
    }

    #[test]
    #[ignore = "only executed in an isolated harness subprocess"]
    fn child_worker() {
        match std::env::var("RUST_DOS_CHECK_WORKER").unwrap().as_str() {
            "ok" => println!("bounded child completed"),
            "fail" => panic!("intentional subprocess failure"),
            "hang" => std::thread::sleep(Duration::from_secs(30)),
            "descendant" => {
                let mut command = worker("hang");
                command.stdout(Stdio::null()).stderr(Stdio::null());
                let mut child = command.spawn().unwrap();
                std::fs::write(
                    std::env::var_os("RUST_DOS_CHECK_PID_FILE").unwrap(),
                    child.id().to_string(),
                )
                .unwrap();
                child.wait().unwrap();
            }
            "flood" => {
                let block = [b'x'; 4096];
                loop {
                    if std::io::stdout().write_all(&block).is_err() {
                        break;
                    }
                }
            }
            _ => panic!("unknown worker mode"),
        }
    }

    #[tokio::test]
    async fn successful_process_executes_nonzero_evidence() {
        let result = run(worker("ok"), Duration::from_secs(5), 65536)
            .await
            .unwrap();
        let text = String::from_utf8(result.bytes).unwrap();
        assert!(text.contains("bounded child completed"));
        assert!(text.contains("1 passed"));
    }

    #[tokio::test]
    async fn known_failing_process_is_not_success() {
        assert!(
            run(worker("fail"), Duration::from_secs(5), 65536)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn missing_tool_fails_closed() {
        assert!(
            run(
                Command::new("rust-dos-mcp-nonexistent-tool"),
                Duration::from_secs(1),
                4096
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn timed_out_child_is_reaped() {
        let start = std::time::Instant::now();
        let result = run(worker("hang"), Duration::from_millis(50), 65536).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[cfg(any(target_os = "linux", windows))]
    #[tokio::test]
    async fn timeout_also_terminates_a_descendant() {
        let temporary = TemporaryCopy::new().unwrap();
        let path = temporary.path.join("child.pid");
        let mut command = worker("descendant");
        command.env("RUST_DOS_CHECK_PID_FILE", &path);
        assert_eq!(
            run(command, Duration::from_secs(2), 65536)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let pid: u32 = std::fs::read_to_string(path).unwrap().parse().unwrap();
        #[cfg(target_os = "linux")]
        {
            let path = format!("/proc/{pid}/stat");
            for _ in 0..100 {
                match std::fs::read_to_string(&path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return,
                    Ok(stat)
                        if stat.rsplit_once(')').unwrap().1.split_whitespace().next()
                            == Some("Z") =>
                    {
                        return;
                    }
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
            panic!("descendant is still running");
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
            use windows_sys::Win32::{Foundation::*, System::Threading::*};
            // SAFETY: query this observed child PID with synchronization-only
            // access; transfer ownership only after the null-handle check.
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            if handle.is_null() {
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(ERROR_INVALID_PARAMETER as i32)
                );
            } else {
                // SAFETY: OpenProcess returned this live, owned handle.
                let owner = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
                // SAFETY: the handle has synchronization rights and remains live.
                assert_eq!(
                    unsafe { WaitForSingleObject(owner.as_raw_handle().cast(), 1000) },
                    WAIT_OBJECT_0
                );
            }
        }
    }

    #[tokio::test]
    async fn fact_inventory_covers_current_contract_and_build_surfaces() {
        let inventory = surface_inventory().await.unwrap();
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "src/args.rs",
            "src/startup.rs",
            "src/debug/control.rs",
            "tests/mcp_foundation.rs",
            "examples/mcp_check.rs",
            ".github/workflows/mcp.yml",
            "docs/mcp/README.md",
        ] {
            assert!(inventory.contains_key(path), "missing surface: {path}");
        }
        assert!(fact_surface("tests/fixtures/approved/mcp/manifest.json"));
        assert!(fact_surface("schemas/mcp.json"));
        assert!(!fact_surface("docs/mcp/captures/private.png"));
        assert!(!fact_surface("docs/mcp/execution/surface-review.json"));
    }

    #[tokio::test]
    async fn excessive_output_terminates_the_child() {
        let result = run(worker("flood"), Duration::from_secs(5), 16384).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn unknown_case_and_extra_arguments_fail_closed() {
        assert!(parse(&["--case", "unknown"]).is_err());
        assert!(parse(&["foundation", "extra"]).is_err());
        assert!(parse(&["phase", "0"]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn implemented_foundation_is_registered() {
        assert!(parse(&["foundation"]).is_ok());
        assert!(parse(&["--case", "holds"]).is_ok());
        assert!(parse(&["phase", "6"]).is_ok());
    }

    #[tokio::test]
    async fn empty_test_filter_cannot_prove_evidence() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "no_such_evidence_case"]);
        let output = run(command, Duration::from_secs(5), 65536).await.unwrap();
        assert!(executed_tests(&output).is_err());
        let output = run(worker("ok"), Duration::from_secs(5), 65536)
            .await
            .unwrap();
        assert_eq!(executed_tests(&output).unwrap(), 1);
    }

    #[test]
    fn passing_tooling_cannot_mask_an_empty_required_suite() {
        let output = Output {
            bytes: b"test result: ok. 0 passed; 0 failed;\ntest result: ok. 18 passed; 0 failed;\n"
                .to_vec(),
            stdout: Vec::new(),
            successful: true,
        };
        assert!(required_test_groups(&output).is_err());
    }

    #[test]
    fn missing_fixture_fails_closed() {
        let temporary = TemporaryCopy::new().unwrap();
        assert_eq!(
            require_file(&temporary.path.join("manifest.json"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn isolated_copy_and_patch_preserve_the_source() {
        let temporary = TemporaryCopy::new().unwrap();
        let original = std::path::Path::new("src/debug/control.rs");
        let before = std::fs::read(original).unwrap();
        let destination = temporary.path.join("control.rs");
        let mut budget = 0;
        copy_source(original, &destination, &mut budget).unwrap();
        patch_unavailable_observation(&destination).unwrap();
        assert_eq!(std::fs::read(original).unwrap(), before);
        assert!(
            !std::fs::read_to_string(&destination)
                .unwrap()
                .contains("Err(ControlError::Unavailable)")
        );
        assert!(patch_unavailable_observation(&destination).is_err());
        let path = temporary.path.clone();
        drop(temporary);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn invalid_process_budgets_fail_before_spawn() {
        let error = run(worker("ok"), Duration::ZERO, 4096).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        let error = run(worker("ok"), Duration::from_secs(5), 0)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn required_records_reject_directories_empty_and_oversized_files() {
        let temporary = TemporaryCopy::new().unwrap();
        assert!(require_file(&temporary.path).is_err());
        let path = temporary.path.join("record.rs");
        std::fs::copy("src/debug/control.rs", &path).unwrap();
        assert!(require_file(&path).is_ok());
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(0).unwrap();
        assert!(require_file(&path).is_err());
        file.set_len(1024 * 1024 + 1).unwrap();
        assert!(require_file(&path).is_err());
    }

    #[test]
    fn source_copy_skips_build_outputs_and_enforces_aggregate_limit() {
        let temporary = TemporaryCopy::new().unwrap();
        let source = temporary.path.join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(source.join("target")).unwrap();
        std::fs::copy("src/debug/control.rs", source.join("control.rs")).unwrap();
        let destination = temporary.path.join("copy");
        let mut budget = 0;
        copy_source(&source, &destination, &mut budget).unwrap();
        assert!(destination.join("control.rs").is_file());
        assert!(!destination.join("target").exists());
        let mut budget = 512 * 1024 * 1024;
        assert_eq!(
            copy_source(
                &source.join("control.rs"),
                &temporary.path.join("over.rs"),
                &mut budget
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::FileTooLarge
        );
        assert!(!temporary.path.join("over.rs").exists());
        temporary.cleanup().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn source_copy_does_not_follow_symlinks() {
        let temporary = TemporaryCopy::new().unwrap();
        let link = temporary.path.join("link");
        std::os::unix::fs::symlink(
            std::env::current_dir()
                .unwrap()
                .join("src/debug/control.rs"),
            &link,
        )
        .unwrap();
        assert!(copy_source(&link, &temporary.path.join("copy"), &mut 0).is_err());
    }

    #[tokio::test]
    async fn real_assets_support_isolated_guest_preparation() {
        assert!(parse(&["assets", "--prepare"]).is_ok());
    }

    #[tokio::test]
    async fn approved_metadata_policy_is_portable() {
        dispatch(parse(&["approvals"]).unwrap()).await.unwrap();
    }

    #[tokio::test]
    async fn missing_docs_and_stale_actual_review_inventory_are_rejected() {
        let temporary = TemporaryCopy::new().unwrap();
        assert!(verify_docs(&temporary.path).is_err());
        let actual = surface_inventory().await.unwrap();
        verify_review_inventory(&actual, &actual).unwrap();
        let mut stale = actual.clone();
        *stale.values_mut().next().unwrap() = String::new();
        assert!(verify_review_inventory(&stale, &actual).is_err());
    }

    #[test]
    fn source_copy_rejects_a_large_file_and_counter_overflow() {
        let temporary = TemporaryCopy::new().unwrap();
        let source = temporary.path.join("control.rs");
        std::fs::copy("src/debug/control.rs", &source).unwrap();
        let destination = temporary.path.join("copy.rs");
        let mut budget = u64::MAX;
        assert!(copy_source(&source, &destination, &mut budget).is_err());
        assert!(!destination.exists());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_len(64 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            copy_source(&source, &destination, &mut 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::FileTooLarge
        );
        assert!(!destination.exists());
    }

    #[test]
    fn cleanup_failure_is_reported_and_drop_does_not_panic() {
        let temporary = TemporaryCopy::new().unwrap();
        std::fs::remove_dir(&temporary.path).unwrap();
        assert_eq!(
            temporary.cleanup().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[tokio::test]
    async fn pinned_toolchain_commands_run_through_rustup() {
        let current = option_env!("RUSTUP_TOOLCHAIN").unwrap_or("1.98.1");
        let argument = format!("+{current}");
        let command = cargo(&[&argument, "--version"]);
        assert_eq!(command.get_program(), "rustup");
        assert!(
            run(command, Duration::from_secs(10), 8192)
                .await
                .unwrap()
                .successful
        );
        cargo_check(&["--version"], false).await.unwrap();
    }
}
