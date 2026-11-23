//! Single owner of every HWND mutation (audit I05).
//!
//! Window callbacks, tray actions, IPC commands and timers never touch native
//! windows directly: they submit intents here. A dedicated thread collects
//! them, coalesces layout requests and hover polls, and hands one batch at a
//! time to the main thread with `run_on_main_thread`. Because that call is
//! made from this thread, the batch is always *posted* to the event loop and
//! runs after the current callback has returned, never inside it (Tauri runs
//! the closure inline when called from the main thread itself). While a batch
//! runs, nothing else is dispatched, so the event loop never accumulates
//! stale work: a busy queue simply merges what arrives meanwhile.
//!
//! Set `AURORA_TRACE=1` to log, per batch, the jobs, the queueing latency, the
//! run time, the executing thread and the callback depth.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::sync::{lock, wait_timeout};

use super::position::Side;

/// Cursor poll cadence while an indicator is open.
pub const POLL: Duration = Duration::from_millis(80);
/// Display topology and work-area check cadence.
pub const TOPOLOGY: Duration = Duration::from_secs(2);

/// Reply channel of a job whose caller waits for the native result.
pub type Reply = tauri::async_runtime::Sender<Result<bool, String>>;

pub enum Job {
    /// Recompute geometry and re-place the rail (DPI, work area, topology).
    Layout,
    /// Bring the rail back if something external minimized it.
    Restore,
    Toggle,
    SetSide(Side),
    Motion {
        generation: u32,
        shifts: [u8; 2],
        reply: Reply,
    },
    ShowSummary {
        revision: u64,
        height: i32,
    },
    /// The open intent with this epoch elapsed.
    Intent(u32),
    HoverTick,
    TopologyTick,
    /// Preferences were saved: apply side, monitor and visible providers.
    Preferences,
    OpenSettings,
}

impl Job {
    fn name(&self) -> &'static str {
        match self {
            Job::Layout => "layout",
            Job::Restore => "restore",
            Job::Toggle => "toggle",
            Job::SetSide(_) => "side",
            Job::Motion { .. } => "motion",
            Job::ShowSummary { .. } => "summary",
            Job::Intent(_) => "intent",
            Job::HoverTick => "hover",
            Job::TopologyTick => "topology",
            Job::Preferences => "preferences",
            Job::OpenSettings => "settings",
        }
    }
}

/// Pending work. Pure state, unit-tested without windows.
#[derive(Default)]
pub struct Queue {
    jobs: VecDeque<Job>,
    layout: bool,
    busy: bool,
    stopped: bool,
    /// One cancelable open deadline for the whole rail (audit I04).
    intent: Option<(u32, Instant)>,
    /// Next cursor poll, while an indicator is open.
    poll: Option<Instant>,
    topology: Option<Instant>,
}

impl Queue {
    fn push(&mut self, job: Job) {
        match job {
            // Layout requests merge: the last geometry wins anyway.
            Job::Layout => self.layout = true,
            job => self.jobs.push_back(job),
        }
    }

    /// The next batch, or the time to wake up and check again.
    fn take(&mut self, now: Instant) -> Result<Vec<Job>, Option<Instant>> {
        if !self.busy {
            let mut batch = Vec::new();
            if std::mem::take(&mut self.layout) {
                batch.push(Job::Layout);
            }
            batch.extend(self.jobs.drain(..));
            if let Some((epoch, deadline)) = self.intent
                && now >= deadline
            {
                self.intent = None;
                batch.push(Job::Intent(epoch));
            }
            if self.poll.is_some_and(|next| now >= next) {
                self.poll = Some(now + POLL);
                batch.push(Job::HoverTick);
            }
            if self.topology.is_some_and(|next| now >= next) {
                self.topology = Some(now + TOPOLOGY);
                batch.push(Job::TopologyTick);
            }
            if !batch.is_empty() {
                self.busy = true;
                return Ok(batch);
            }
        }
        Err([self.intent.map(|(_, at)| at), self.poll, self.topology]
            .into_iter()
            .flatten()
            .min())
    }
}

#[derive(Default)]
pub struct Controller {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Controller {
    pub fn submit(&self, job: Job) {
        lock(&self.queue).push(job);
        self.wake.notify_one();
    }

    /// Starts or stops the cursor poll.
    pub fn watch(&self, active: bool) {
        lock(&self.queue).poll = active.then(|| Instant::now() + POLL);
        self.wake.notify_one();
    }

    /// Replaces the pending open intent: only the latest one can fire.
    pub fn intent(&self, epoch: u32, delay: Duration) {
        lock(&self.queue).intent = Some((epoch, Instant::now() + delay));
        self.wake.notify_one();
    }

    #[cfg(test)]
    pub fn cancel_intent(&self) {
        lock(&self.queue).intent = None;
    }

    /// Starts the periodic work-area and topology check.
    pub fn watch_topology(&self) {
        lock(&self.queue).topology = Some(Instant::now() + TOPOLOGY);
        self.wake.notify_one();
    }

    fn done(&self) {
        lock(&self.queue).busy = false;
        self.wake.notify_one();
    }

    fn stop(&self) {
        lock(&self.queue).stopped = true;
        self.wake.notify_one();
    }

    /// Blocks until a batch is ready, or returns `None` once stopped.
    fn next(&self) -> Option<Vec<Job>> {
        let mut queue = lock(&self.queue);
        loop {
            if queue.stopped {
                return None;
            }
            match queue.take(Instant::now()) {
                Ok(batch) => return Some(batch),
                Err(deadline) => {
                    // While a batch runs, deadlines wait for `done`.
                    let idle = Duration::from_secs(3600);
                    let wait = match deadline {
                        Some(at) if !queue.busy => at
                            .saturating_duration_since(Instant::now())
                            .max(Duration::from_millis(1)),
                        _ => idle,
                    };
                    // Every submit, deadline change or `done` notifies; the
                    // loop re-evaluates after any wake-up.
                    queue = wait_timeout(&self.wake, queue, wait);
                }
            }
        }
    }
}

thread_local! {
    /// Nesting depth of batch execution on this thread. Always 0 on entry,
    /// unless a native call re-entered the event loop (logged when tracing).
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn tracing() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("AURORA_TRACE").is_some_and(|value| value == "1"))
}

/// Runs the dispatch loop. `execute` runs on the main thread, one batch at a
/// time, and must not block on this controller.
pub fn run(
    app: tauri::AppHandle,
    controller: Arc<Controller>,
    execute: fn(&tauri::AppHandle, Vec<Job>),
) {
    let main = std::sync::Mutex::new(None::<std::thread::ThreadId>);
    let main = Arc::new(main);
    while let Some(batch) = controller.next() {
        let queued = Instant::now();
        let names: Vec<&'static str> = batch.iter().map(Job::name).collect();
        let handle = app.clone();
        let owner = Arc::clone(&controller);
        let main = Arc::clone(&main);
        let posted = app.run_on_main_thread(move || {
            let started = Instant::now();
            let depth = DEPTH.with(|depth| {
                depth.set(depth.get() + 1);
                depth.get()
            });
            let thread = std::thread::current().id();
            let owner_thread = *lock(&main).get_or_insert(thread);
            // A panic in one batch must not leave the queue busy forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&handle, batch);
            }));
            DEPTH.with(|depth| depth.set(depth.get() - 1));
            owner.done();
            if result.is_err() {
                eprintln!("Aurora Control: a window batch panicked");
            }
            if tracing() || depth != 1 || owner_thread != thread {
                eprintln!(
                    "[aurora] batch {names:?} waited {:?} ran {:?} thread {thread:?} owner {owner_thread:?} depth {depth}",
                    started.duration_since(queued),
                    started.elapsed(),
                );
            }
        });
        if posted.is_err() {
            controller.stop();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(batch: &[Job]) -> Vec<&'static str> {
        batch.iter().map(Job::name).collect()
    }

    #[test]
    fn i05_layout_requests_merge_and_run_first() {
        let mut queue = Queue::default();
        let start = Instant::now();
        queue.push(Job::Toggle);
        queue.push(Job::Layout);
        queue.push(Job::Layout);
        queue.push(Job::Layout);
        let batch = queue.take(start).ok().unwrap();
        assert_eq!(names(&batch), ["layout", "toggle"]);
    }

    #[test]
    fn i05_nothing_is_dispatched_while_a_batch_runs() {
        let mut queue = Queue::default();
        let start = Instant::now();
        queue.push(Job::Toggle);
        assert!(queue.take(start).is_ok());
        // Callbacks during the batch only queue intents.
        queue.push(Job::Layout);
        queue.poll = Some(start);
        queue.push(Job::Layout);
        assert!(queue.take(start + POLL).is_err(), "busy");
        queue.busy = false;
        let batch = queue.take(start + POLL * 3).ok().unwrap();
        // Several missed polls become one tick; two layouts become one.
        assert_eq!(names(&batch), ["layout", "hover"]);
    }

    #[test]
    fn i04_a_single_cancelable_intent_per_rail() {
        let controller = Controller::default();
        controller.intent(1, Duration::from_millis(150));
        controller.intent(2, Duration::from_millis(150));
        let mut queue = lock(&controller.queue);
        let now = Instant::now();
        assert!(queue.take(now).is_err(), "not due yet");
        let batch = queue.take(now + Duration::from_millis(200)).ok().unwrap();
        assert!(
            matches!(batch.as_slice(), [Job::Intent(2)]),
            "only the latest"
        );
        queue.busy = false;
        drop(queue);
        controller.intent(3, Duration::ZERO);
        controller.cancel_intent();
        let mut queue = lock(&controller.queue);
        assert!(queue.take(Instant::now() + Duration::from_secs(1)).is_err());
    }

    #[test]
    fn i05_deadlines_wake_the_dispatcher() {
        let mut queue = Queue::default();
        let start = Instant::now();
        queue.topology = Some(start + TOPOLOGY);
        queue.poll = Some(start + POLL);
        assert_eq!(queue.take(start).err(), Some(Some(start + POLL)));
        queue.poll = None;
        assert_eq!(queue.take(start).err(), Some(Some(start + TOPOLOGY)));
    }
}
