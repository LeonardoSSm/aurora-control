//! Controlled subprocesses for official CLIs (Antigravity).
//!
//! One attempt runs one executable with fixed arguments and must never
//! outlive its reason to exist:
//! - [`Control`] is shared with the thread that changes the preferences.
//!   `cancel` bumps an epoch and kills the running process tree at once; an
//!   attempt whose epoch changed is void, even if the provider was enabled
//!   again in the meantime. The check of the cancel flag and the spawn happen
//!   under the same lock as `cancel`, so a disable can never slip between them.
//! - On Windows every attempt gets a Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: descendants the CLI starts are
//!   killed with it, when it times out, when it is cancelled, after it exits,
//!   and when Aurora Control itself exits (the system closes the handle).
//!   Leftover descendants can therefore never hold the output pipes open.
//! - Output is read by two threads, each bounded in size; they are awaited
//!   for a short grace period only, after the tree is gone.
//!
//! Known limit: the process is assigned to its job right after `spawn`
//! returns. A child started by the CLI within those microseconds would not
//! be in the job; the official CLI needs far longer to start its runtime.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::sync::lock;

/// What a finished run produced.
#[derive(Clone, Debug, PartialEq)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunError {
    /// The executable is not installed.
    Missing,
    /// It could not be started.
    Spawn,
    Timeout,
    /// More output than any report needs.
    TooLarge,
    /// The provider was disabled before or during the run.
    Cancelled,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub max_output: usize,
}

/// How long the output readers may take once the process tree is gone.
const READER_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(25);

#[derive(Default)]
struct State {
    cancelled: bool,
    epoch: u64,
    job: Option<Arc<Job>>,
}

/// Cancellation shared by the worker and the preferences thread.
#[derive(Default)]
pub struct Control {
    state: Mutex<State>,
}

impl Control {
    pub fn new() -> Self {
        Self::default()
    }

    /// The provider was disabled: no new run starts, and the running one (with
    /// everything it started) is killed now. Never blocks on the run itself.
    pub fn cancel(&self) {
        let job = {
            let mut state = lock(&self.state);
            state.cancelled = true;
            state.epoch = state.epoch.wrapping_add(1);
            state.job.clone()
        };
        if let Some(job) = job {
            job.terminate();
        }
    }

    /// The provider was enabled again. Runs cancelled before stay void.
    pub fn resume(&self) {
        lock(&self.state).cancelled = false;
    }

    #[cfg(test)]
    pub fn is_cancelled(&self) -> bool {
        lock(&self.state).cancelled
    }

    fn epoch(&self) -> u64 {
        lock(&self.state).epoch
    }
}

/// Reads at most `limit` bytes from `reader`, flagging anything beyond.
pub fn drain(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut over = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if buffer.len() + read > limit {
                    over = true;
                    // Keep reading (and discarding) so the child never blocks
                    // on a full pipe before it is stopped.
                    continue;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
        }
    }
    (buffer, over)
}

fn reader(stream: impl Read + Send + 'static, limit: usize) -> mpsc::Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    // Without a thread the sender is dropped: the receiver reports a
    // disconnect and the run fails instead of waiting.
    let _ = std::thread::Builder::new()
        .name("cli-output".into())
        .spawn(move || {
            let _ = sender.send(drain(stream, limit));
        });
    receiver
}

/// Runs `command` (stdin closed, output captured) under `control` and
/// `limits`. The whole process tree is gone when this returns.
pub fn run(mut command: Command, control: &Control, limits: Limits) -> Result<CliOutput, RunError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes on the user's screen.
        command.creation_flags(0x0800_0000);
    }

    // Checked and spawned under the lock `cancel` takes: a disable either
    // happens before (nothing starts) or after (the job exists to be killed).
    let (mut child, job, epoch) = {
        let mut state = lock(&control.state);
        if state.cancelled {
            return Err(RunError::Cancelled);
        }
        let child = command.spawn().map_err(|_| RunError::Spawn)?;
        let job = Job::for_child(&child).map(Arc::new);
        state.job.clone_from(&job);
        (child, job, state.epoch)
    };

    let finish = |child: &mut Child| {
        if let Some(job) = &job {
            job.terminate();
        }
        let _ = child.kill();
        let _ = child.wait();
        let mut state = lock(&control.state);
        if state
            .job
            .as_ref()
            .zip(job.as_ref())
            .is_some_and(|(current, ours)| Arc::ptr_eq(current, ours))
        {
            state.job = None;
        }
    };

    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        finish(&mut child);
        return Err(RunError::Spawn);
    };
    let out = reader(stdout, limits.max_output);
    let err = reader(stderr, limits.max_output);

    let started = Instant::now();
    let status = loop {
        if control.epoch() != epoch {
            break Err(RunError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= limits.timeout => break Err(RunError::Timeout),
            Ok(None) => std::thread::sleep(POLL),
            Err(_) => break Err(RunError::Spawn),
        }
    };
    // Whatever happened, nothing the CLI started outlives the attempt.
    finish(&mut child);
    let status = status?;
    // Disabled while it ran (even if enabled again since): the result
    // belongs to an attempt that no longer exists.
    if control.epoch() != epoch {
        return Err(RunError::Cancelled);
    }
    let (stdout, over_out) = out
        .recv_timeout(READER_GRACE)
        .map_err(|_| RunError::Timeout)?;
    let (stderr, _) = err.recv_timeout(READER_GRACE).unwrap_or_default();
    if over_out {
        return Err(RunError::TooLarge);
    }
    Ok(CliOutput {
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// A Windows Job Object that kills its processes when terminated or closed.
pub struct Job {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: a job handle is a kernel object handle; the calls made on it
// (`TerminateJobObject`, `CloseHandle`) are thread-safe.
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

impl Job {
    /// A new job holding `child`. `None` when the system refuses (the run
    /// then relies on killing the child alone).
    #[cfg(windows)]
    fn for_child(child: &Child) -> Option<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW,
            JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: plain Win32 calls with valid arguments; every failure path
        // closes the handle it created.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            let configured = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if configured == 0 || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0 {
                CloseHandle(handle);
                return None;
            }
            Some(Self { handle })
        }
    }

    #[cfg(not(windows))]
    fn for_child(_child: &Child) -> Option<Self> {
        None
    }

    fn terminate(&self) {
        #[cfg(windows)]
        // SAFETY: the handle is valid until `drop`.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: closed exactly once; KILL_ON_JOB_CLOSE ends what is left.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        timeout: Duration::from_secs(20),
        max_output: 64 * 1024,
    };

    fn cmd(script: &str) -> Command {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd.exe");
        // cmd.exe parses its own command line: pass the script verbatim.
        command.args(["/d", "/c"]).raw_arg(script);
        command
    }

    #[test]
    fn a_normal_run_returns_its_output_and_exit_code() {
        let output = run(cmd("echo hello& exit /b 3"), &Control::new(), LIMITS).unwrap();
        assert_eq!(output.code, Some(3));
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[test]
    fn a_descendant_holding_the_pipes_does_not_hold_the_run() {
        // cmd exits at once, but the grandchild inherits stdout and would
        // keep the pipe open for 30 s without the job.
        let started = Instant::now();
        let output = run(
            cmd("start /b ping -n 30 127.0.0.1 >nul & echo done"),
            &Control::new(),
            LIMITS,
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(output.unwrap().stdout.trim(), "done");
    }

    #[test]
    fn the_timeout_kills_the_whole_tree() {
        let started = Instant::now();
        let limits = Limits {
            timeout: Duration::from_millis(500),
            ..LIMITS
        };
        let result = run(cmd("ping -n 30 127.0.0.1 >nul"), &Control::new(), limits);
        assert_eq!(result, Err(RunError::Timeout));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_cancel_before_the_spawn_starts_nothing() {
        let dir = crate::usage::testing::TempDir::new("process-cancel");
        let marker = dir.path().join("started");
        let control = Control::new();
        control.cancel();
        let script = format!("echo x > \"{}\"", marker.display());
        assert_eq!(
            run(cmd(&script), &control, LIMITS),
            Err(RunError::Cancelled)
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists(), "the process must never start");
        // Enabled again: the next run starts.
        control.resume();
        assert!(run(cmd(&script), &control, LIMITS).is_ok());
        assert!(marker.exists());
    }

    #[test]
    fn a_cancel_during_the_run_kills_it_even_if_enabled_again_at_once() {
        let control = Arc::new(Control::new());
        let worker = {
            let control = Arc::clone(&control);
            std::thread::spawn(move || {
                let started = Instant::now();
                (
                    run(cmd("ping -n 30 127.0.0.1 >nul"), &control, LIMITS),
                    started.elapsed(),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(400));
        // A quick disable and re-enable, faster than any poll.
        control.cancel();
        control.resume();
        let (result, elapsed) = worker.join().unwrap();
        assert_eq!(result, Err(RunError::Cancelled));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert!(!control.is_cancelled());
        // The control is usable again.
        assert!(run(cmd("echo ok"), &control, LIMITS).is_ok());
    }

    #[test]
    fn output_beyond_the_limit_is_refused() {
        let limits = Limits {
            max_output: 16,
            ..LIMITS
        };
        assert_eq!(
            run(
                cmd("echo 0123456789012345678901234567890123456789"),
                &Control::new(),
                limits
            ),
            Err(RunError::TooLarge)
        );
    }

    #[test]
    fn drain_stops_collecting_past_the_limit() {
        let (bytes, over) = drain(&b"abcdef"[..], 4);
        assert!(over);
        assert!(bytes.len() <= 4);
        let (bytes, over) = drain(&b"abc"[..], 4);
        assert_eq!((bytes.as_slice(), over), (&b"abc"[..], false));
    }
}
