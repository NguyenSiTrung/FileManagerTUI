//! Bounded, generation-aware collaboration primitives.
//! Runtime transport migration is separate from this reusable scheduling core.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;

/// Account for retained payload allocation (capacity, not only length).
/// Implementations must include owned nested allocations. Worker-internal I/O
/// must independently apply its document/clipboard/scan limits.
pub trait Payload {
    fn payload_bytes(&self) -> usize;
}

impl Payload for Vec<u8> {
    fn payload_bytes(&self) -> usize {
        self.capacity()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub queued_jobs: usize,
    pub queued_results: usize,
    pub workers: usize,
    pub request_domains: usize,
    pub job_bytes: usize,
    pub result_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            queued_jobs: 16,
            queued_results: 16,
            workers: 2,
            request_domains: 32,
            job_bytes: 1024 * 1024,
            result_bytes: 8 * 1024 * 1024,
        }
    }
}

pub type AsyncWork<I, O> =
    dyn Fn(I, Cancellation) -> Pin<Box<dyn Future<Output = O> + Send>> + Send + Sync;

pub enum Worker<I, O> {
    Blocking(Arc<dyn Fn(I, Cancellation) -> O + Send + Sync>),
    Async(Arc<AsyncWork<I, O>>),
}

impl<I, O> Clone for Worker<I, O> {
    fn clone(&self) -> Self {
        match self {
            Self::Blocking(work) => Self::Blocking(work.clone()),
            Self::Async(work) => Self::Async(work.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    Full,
    Closed,
    DomainLimit,
    PayloadTooLarge,
    GenerationExhausted,
}

pub struct Rejected<I> {
    pub reason: AdmissionError,
    pub input: I,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobError {
    Panicked,
    PayloadTooLarge,
}

#[derive(Debug)]
pub struct Completion<O> {
    pub generation: RequestGeneration,
    pub result: Result<O, JobError>,
}

/// Bounded reusable core. Not yet connected to application Event/PTY transport.
pub struct Scheduler<I, O> {
    jobs: mpsc::Sender<Job<I>>,
    results: mpsc::Receiver<Completion<O>>,
    requests: Arc<Mutex<Requests>>,
    stop: Cancellation,
    workers: Vec<JoinHandle<()>>,
    limits: Limits,
}

impl<I: Payload + Send + 'static, O: Payload + Send + 'static> Scheduler<I, O> {
    /// Creates exactly `workers` orchestration tasks, each owning at most one
    /// running job. No task is spawned per queued/rejected submission.
    pub fn new(limits: Limits, worker: Worker<I, O>) -> Result<Self, &'static str> {
        if limits.queued_jobs == 0
            || limits.queued_results == 0
            || limits.workers == 0
            || limits.workers > 64
            || limits.request_domains == 0
            || limits.job_bytes == 0
            || limits.result_bytes == 0
            || limits.queued_jobs > tokio::sync::Semaphore::MAX_PERMITS
            || limits.queued_results > tokio::sync::Semaphore::MAX_PERMITS
        {
            return Err("Invalid background limits");
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "Background scheduler requires a Tokio runtime")?;
        let (jobs, rx) = mpsc::channel(limits.queued_jobs);
        let (tx, results) = mpsc::channel(limits.queued_results);
        let requests = Arc::new(Mutex::new(Requests::new(limits.request_domains)));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let stop = Cancellation::new();
        let mut workers = Vec::with_capacity(limits.workers);
        for _ in 0..limits.workers {
            workers.push(runtime.spawn(run_worker(
                rx.clone(),
                tx.clone(),
                worker.clone(),
                stop.clone(),
                limits.result_bytes,
            )));
        }
        Ok(Self {
            jobs,
            results,
            requests,
            stop,
            workers,
            limits,
        })
    }

    /// Nonblocking UI admission. Rejection returns the entire input unchanged.
    /// Capacity is reserved before superseding a current domain, so failed
    /// admission never cancels its previous request.
    pub fn try_submit(
        &mut self,
        domain: RequestDomain,
        input: I,
    ) -> Result<RequestGeneration, Rejected<I>> {
        let reject = |reason, input| Err(Rejected { reason, input });
        if self.stop.is_cancelled() {
            return reject(AdmissionError::Closed, input);
        }
        if input.payload_bytes() > self.limits.job_bytes {
            return reject(AdmissionError::PayloadTooLarge, input);
        }
        let permit = match self.jobs.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(_)) => return reject(AdmissionError::Full, input),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return reject(AdmissionError::Closed, input)
            }
        };
        let mut requests = self.requests.lock().expect("request lock");
        let generation = match requests.begin(domain) {
            Ok(generation) => generation,
            Err(reason) => return reject(reason, input),
        };
        let cancellation = requests.current[&domain].cancellation.clone();
        permit.send(Job {
            generation,
            cancellation,
            input,
        });
        Ok(generation)
    }

    pub fn accepts(&self, generation: RequestGeneration) -> bool {
        self.requests
            .lock()
            .expect("request lock")
            .accepts(generation)
    }

    pub fn cancel(&mut self, generation: RequestGeneration) -> bool {
        self.requests
            .lock()
            .expect("request lock")
            .cancel(generation)
    }

    /// Rejects late queued results at consumption, not only at publication.
    /// Delivery retires the generation; the returned completion is the accepted
    /// value to apply, and `accepts` will no longer accept duplicates.
    pub async fn next_result(&mut self) -> Option<Completion<O>> {
        while let Some(completion) = self.results.recv().await {
            if self.cancel(completion.generation) {
                return Some(completion);
            }
        }
        None
    }

    /// Cancel and join workers, waking full-result-queue producers. Blocking
    /// work must observe its token: Rust cannot safely preempt arbitrary I/O.
    /// Cancelling this shutdown future is safe; a later call joins the remainder.
    pub async fn shutdown(&mut self) {
        self.close();
        while let Some(worker) = self.workers.last_mut() {
            let _ = worker.await;
            self.workers.pop();
        }
    }
}

impl<I, O> Scheduler<I, O> {
    fn close(&mut self) {
        self.stop.cancel();
        self.results.close();
        self.requests.lock().expect("request lock").close();
    }
}

impl<I, O> Drop for Scheduler<I, O> {
    fn drop(&mut self) {
        // No UI-thread join. Detached orchestration exits after its cooperative
        // blocking job finishes; it can no longer admit or publish work.
        self.close();
    }
}

struct Job<I> {
    generation: RequestGeneration,
    cancellation: Cancellation,
    input: I,
}

async fn run_worker<I: Send + 'static, O: Payload + Send + 'static>(
    jobs: Arc<tokio::sync::Mutex<mpsc::Receiver<Job<I>>>>,
    results: mpsc::Sender<Completion<O>>,
    worker: Worker<I, O>,
    stop: Cancellation,
    result_bytes: usize,
) {
    loop {
        let job = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            _ = results.closed() => break,
            job = async { jobs.lock().await.recv().await } => match job {
                Some(job) => job,
                None => break,
            },
        };
        let Job {
            generation,
            cancellation,
            input,
        } = job;
        if cancellation.is_cancelled() {
            continue;
        }
        let mut running = match &worker {
            Worker::Blocking(work) => {
                let work = work.clone();
                let token = cancellation.clone();
                // Await even a cancelled blocking job before reusing its slot.
                tokio::task::spawn_blocking(move || {
                    bounded_result(work(input, token), result_bytes)
                })
            }
            Worker::Async(work) => {
                let work = work.clone();
                let token = cancellation.clone();
                // Invoke inside the task, so factory panics also become errors.
                tokio::spawn(async move { bounded_result(work(input, token).await, result_bytes) })
            }
        };
        let result = if matches!(worker, Worker::Async(_)) {
            tokio::select! {
                biased;
                _ = stop.cancelled() => {
                    running.abort();
                    let _ = running.await;
                    break;
                },
                _ = cancellation.cancelled() => {
                    running.abort();
                    let _ = running.await;
                    continue;
                },
                result = &mut running => result,
            }
        } else {
            running.await
        };
        if stop.is_cancelled() {
            break;
        }
        if cancellation.is_cancelled() {
            continue;
        }
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(JobError::Panicked),
        };
        // At most one pending result per worker in addition to the bounded
        // result queue. Cancellation and close both wake backpressured workers.
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            _ = cancellation.cancelled() => {},
            sent = results.send(Completion { generation, result }) => {
                if sent.is_err() { break; }
            },
        }
    }
}

fn bounded_result<O: Payload>(value: O, result_bytes: usize) -> Result<O, JobError> {
    if value.payload_bytes() <= result_bytes {
        Ok(value)
    } else {
        Err(JobError::PayloadTooLarge)
    }
}

/// A caller-owned domain identifier. Different domains never supersede each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestDomain(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestGeneration {
    owner: u64,
    sequence: u64,
}

#[derive(Clone)]
pub struct Cancellation(Arc<CancelState>);

struct CancelState {
    cancelled: AtomicBool,
    wake: Notify,
}

impl Cancellation {
    fn new() -> Self {
        Self(Arc::new(CancelState {
            cancelled: AtomicBool::new(false),
            wake: Notify::new(),
        }))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.wake.notify_waiters();
    }

    pub async fn cancelled(&self) {
        let notified = self.0.wake.notified();
        tokio::pin!(notified);
        // Register before checking to avoid losing a concurrent cancellation.
        notified.as_mut().enable();
        if !self.is_cancelled() {
            notified.await;
        }
    }
}

struct Request {
    generation: RequestGeneration,
    cancellation: Cancellation,
}

struct Requests {
    current: HashMap<RequestDomain, Request>,
    next: u64,
    limit: usize,
    owner: u64,
    closed: bool,
}

impl Requests {
    fn new(limit: usize) -> Self {
        static OWNER: AtomicU64 = AtomicU64::new(1);
        let owner = OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("scheduler identity exhausted");
        Self {
            current: HashMap::new(),
            next: 0,
            limit,
            owner,
            closed: false,
        }
    }

    fn accepts(&self, generation: RequestGeneration) -> bool {
        self.current
            .values()
            .any(|request| request.generation == generation && !request.cancellation.is_cancelled())
    }

    fn begin(&mut self, domain: RequestDomain) -> Result<RequestGeneration, AdmissionError> {
        if self.closed {
            return Err(AdmissionError::Closed);
        }
        if !self.current.contains_key(&domain) && self.current.len() == self.limit {
            return Err(AdmissionError::DomainLimit);
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or(AdmissionError::GenerationExhausted)?;
        let generation = RequestGeneration {
            owner: self.owner,
            sequence: self.next,
        };
        if let Some(old) = self.current.insert(
            domain,
            Request {
                generation,
                cancellation: Cancellation::new(),
            },
        ) {
            old.cancellation.cancel();
        }
        Ok(generation)
    }

    fn cancel(&mut self, generation: RequestGeneration) -> bool {
        let domain = self
            .current
            .iter()
            .find_map(|(&domain, request)| (request.generation == generation).then_some(domain));
        if let Some(domain) = domain {
            if let Some(request) = self.current.remove(&domain) {
                request.cancellation.cancel();
                return true;
            }
        }
        false
    }

    fn close(&mut self) {
        self.closed = true;
        for (_, request) in self.current.drain() {
            request.cancellation.cancel();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RedrawLimits {
    pub frame_interval: std::time::Duration,
    pub event_budget: usize,
}

impl Default for RedrawLimits {
    fn default() -> Self {
        Self {
            frame_interval: std::time::Duration::from_millis(16),
            event_budget: 64,
        }
    }
}

/// Pure dirty-frame scheduling with constructor-injected monotonic time.
/// The consumer must render pending geometry before a mouse hit test.
pub struct DirtyRedraw {
    limits: RedrawLimits,
    last_frame: std::time::Duration,
    dirty: bool,
    immediate: bool,
    events: usize,
}

impl DirtyRedraw {
    pub fn new(now: std::time::Duration, limits: RedrawLimits) -> Result<Self, &'static str> {
        if limits.frame_interval.is_zero() || limits.event_budget == 0 {
            return Err("Invalid redraw limits");
        }
        Ok(Self {
            limits,
            last_frame: now,
            dirty: true,
            immediate: true,
            events: 0,
        })
    }

    /// Input/resize are immediate; terminal/background output may be batched.
    pub fn dirty(&mut self, immediate: bool) {
        self.dirty = true;
        self.immediate |= immediate;
        self.events = self.events.saturating_add(1).min(self.limits.event_budget);
    }

    /// None means await an event, not a zero-deadline idle busy loop.
    pub fn wait(&self, now: std::time::Duration) -> Option<std::time::Duration> {
        if !self.dirty {
            return None;
        }
        if self.immediate || self.events == self.limits.event_budget {
            return Some(std::time::Duration::ZERO);
        }
        Some(
            self.limits
                .frame_interval
                .saturating_sub(now.saturating_sub(self.last_frame)),
        )
    }

    pub fn drawn(&mut self, now: std::time::Duration) {
        self.last_frame = self.last_frame.max(now);
        self.dirty = false;
        self.immediate = false;
        self.events = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(registry: &mut Requests, domain: RequestDomain) -> RequestGeneration {
        registry.begin(domain).unwrap()
    }

    #[test]
    fn background_old_selection_cannot_accept_a_late_result() {
        let mut registry = Requests::new(2);
        let old = request(&mut registry, RequestDomain(1));
        let unrelated = request(&mut registry, RequestDomain(2));
        let current = request(&mut registry, RequestDomain(1));
        assert!(!registry.accepts(old));
        assert!(registry.accepts(current));
        assert!(registry.accepts(unrelated));
    }

    #[test]
    fn background_repeated_cancel_and_closed_requests_release_metadata() {
        let mut registry = Requests::new(1);
        for _ in 0..1000 {
            let generation = request(&mut registry, RequestDomain(1));
            assert!(registry.cancel(generation));
            assert!(!registry.cancel(generation));
            assert!(!registry.accepts(generation));
            assert!(registry.current.is_empty());
        }
    }

    #[test]
    fn background_domain_admission_preserves_existing_request_when_full() {
        let mut registry = Requests::new(1);
        let current = request(&mut registry, RequestDomain(1));
        assert!(registry.begin(RequestDomain(2)).is_err());
        assert!(registry.accepts(current));
    }

    struct Input {
        bytes: Vec<u8>,
        entered: Option<mpsc::Sender<()>>,
        gate: Option<std::sync::mpsc::Receiver<()>>,
        panic: bool,
    }

    impl Payload for Input {
        fn payload_bytes(&self) -> usize {
            self.bytes.capacity()
        }
    }

    fn input(value: u8) -> Input {
        Input {
            bytes: vec![value],
            entered: None,
            gate: None,
            panic: false,
        }
    }

    fn blocking_worker() -> Worker<Input, Vec<u8>> {
        Worker::Blocking(Arc::new(|input: Input, _cancel| {
            if let Some(entered) = input.entered {
                entered.blocking_send(()).unwrap();
            }
            if let Some(gate) = input.gate {
                gate.recv().unwrap();
            }
            assert!(!input.panic, "injected job panic");
            input.bytes
        }))
    }

    fn limits() -> Limits {
        Limits {
            queued_jobs: 2,
            queued_results: 2,
            workers: 2,
            request_domains: 8,
            job_bytes: 128,
            result_bytes: 64,
        }
    }

    fn submit(
        scheduler: &mut Scheduler<Input, Vec<u8>>,
        domain: u64,
        input: Input,
    ) -> RequestGeneration {
        scheduler
            .try_submit(RequestDomain(domain), input)
            .map_err(|e| e.reason)
            .unwrap()
    }

    #[tokio::test]
    async fn background_admission_bounds_queued_and_running_jobs_without_spawning_waiters() {
        let mut scheduler = Scheduler::new(limits(), blocking_worker()).unwrap();
        let (entered, mut ready) = mpsc::channel(2);
        let mut gates = vec![];
        for domain in 1..=2 {
            let (release, gate) = std::sync::mpsc::sync_channel(1);
            let mut job = input(domain as u8);
            job.entered = Some(entered.clone());
            job.gate = Some(gate);
            submit(&mut scheduler, domain, job);
            gates.push(release);
        }
        ready.recv().await.unwrap();
        ready.recv().await.unwrap();
        submit(&mut scheduler, 3, input(3));
        submit(&mut scheduler, 4, input(4));
        let refused = scheduler
            .try_submit(RequestDomain(5), input(5))
            .err()
            .unwrap();
        assert_eq!(refused.reason, AdmissionError::Full);
        assert_eq!(refused.input.bytes, [5]);
        for gate in gates {
            gate.send(()).unwrap();
        }
        let mut results = vec![];
        for _ in 0..4 {
            results.extend(scheduler.next_result().await.unwrap().result.unwrap());
        }
        results.sort();
        assert_eq!(results, [1, 2, 3, 4]);
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_delayed_old_job_is_rejected_without_invalidating_other_domain() {
        let mut scheduler = Scheduler::new(limits(), blocking_worker()).unwrap();
        let (entered, mut ready) = mpsc::channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
        let mut old = input(1);
        old.entered = Some(entered);
        old.gate = Some(gate);
        let old = submit(&mut scheduler, 1, old);
        ready.recv().await.unwrap();
        let other = submit(&mut scheduler, 2, input(2));
        let current = submit(&mut scheduler, 1, input(3));
        release.send(()).unwrap();
        assert!(!scheduler.accepts(old));
        assert!(scheduler.accepts(other));
        assert!(scheduler.accepts(current));
        let mut results = vec![];
        for _ in 0..2 {
            results.extend(scheduler.next_result().await.unwrap().result.unwrap());
        }
        results.sort();
        assert_eq!(results, [2, 3]);
        assert!(!scheduler.accepts(current));
        assert!(!scheduler.cancel(old));
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_worker_panic_reports_failure_and_reuses_capacity() {
        let mut scheduler = Scheduler::new(limits(), blocking_worker()).unwrap();
        let mut job = input(1);
        job.panic = true;
        let generation = submit(&mut scheduler, 1, job);
        let result = scheduler.next_result().await.unwrap();
        assert_eq!(result.generation, generation);
        assert_eq!(result.result, Err(JobError::Panicked));
        submit(&mut scheduler, 2, input(2));
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![2]));
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_payload_limit_rejects_whole_allocation_not_truncated_user_data() {
        let mut scheduler = Scheduler::new(limits(), blocking_worker()).unwrap();
        let mut oversized = input(7);
        oversized.bytes = Vec::with_capacity(129);
        oversized.bytes.push(7);
        let refused = scheduler
            .try_submit(RequestDomain(1), oversized)
            .err()
            .unwrap();
        assert_eq!(refused.reason, AdmissionError::PayloadTooLarge);
        assert_eq!(refused.input.bytes, [7]);
        submit(
            &mut scheduler,
            2,
            Input {
                bytes: vec![8; 65],
                ..input(8)
            },
        );
        assert_eq!(
            scheduler.next_result().await.unwrap().result,
            Err(JobError::PayloadTooLarge)
        );
        submit(&mut scheduler, 3, input(3));
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![3]));
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_shutdown_closes_admission_and_result_stream() {
        let mut scheduler = Scheduler::new(limits(), blocking_worker()).unwrap();
        scheduler.shutdown().await;
        let refused = scheduler
            .try_submit(RequestDomain(1), input(1))
            .err()
            .unwrap();
        assert_eq!(refused.reason, AdmissionError::Closed);
        assert!(scheduler.next_result().await.is_none());
        scheduler.shutdown().await;
    }

    #[test]
    fn background_generations_from_other_schedulers_are_never_accepted() {
        let mut first = Requests::new(1);
        let mut second = Requests::new(1);
        let one = request(&mut first, RequestDomain(1));
        let two = request(&mut second, RequestDomain(1));
        assert!(first.accepts(one));
        assert!(!first.accepts(two));
        assert!(!second.cancel(one));
        assert!(second.accepts(two));
    }

    #[test]
    fn background_superseded_running_job_observes_its_own_cancellation() {
        let mut registry = Requests::new(1);
        request(&mut registry, RequestDomain(1));
        let old_token = registry.current[&RequestDomain(1)].cancellation.clone();
        request(&mut registry, RequestDomain(1));
        assert!(old_token.is_cancelled());
        assert!(!registry.current[&RequestDomain(1)]
            .cancellation
            .is_cancelled());
    }

    #[tokio::test]
    async fn background_async_cancel_aborts_job_and_releases_worker_slot() {
        let (entered, mut ready) = mpsc::channel(1);
        let worker = Worker::Async(Arc::new(move |bytes: Vec<u8>, _cancel| {
            let entered = entered.clone();
            Box::pin(async move {
                if bytes == [1] {
                    entered.send(()).await.unwrap();
                    std::future::pending::<()>().await;
                }
                bytes
            }) as Pin<Box<dyn Future<Output = Vec<u8>> + Send>>
        }));
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            worker,
        )
        .unwrap();
        let old = scheduler
            .try_submit(RequestDomain(1), vec![1])
            .map_err(|e| e.reason)
            .unwrap();
        ready.recv().await.unwrap();
        assert!(scheduler.cancel(old));
        assert!(!scheduler.cancel(old));
        let current = scheduler
            .try_submit(RequestDomain(1), vec![2])
            .map_err(|e| e.reason)
            .unwrap();
        let result = scheduler.next_result().await.unwrap();
        assert_eq!(result.generation, current);
        assert_eq!(result.result, Ok(vec![2]));
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_invalid_limits_fail_instead_of_panicking_or_spawning() {
        for invalid in [
            Limits {
                queued_jobs: 0,
                ..limits()
            },
            Limits {
                queued_results: 0,
                ..limits()
            },
            Limits {
                workers: 0,
                ..limits()
            },
            Limits {
                workers: 65,
                ..limits()
            },
            Limits {
                request_domains: 0,
                ..limits()
            },
            Limits {
                job_bytes: 0,
                ..limits()
            },
            Limits {
                result_bytes: 0,
                ..limits()
            },
            Limits {
                queued_jobs: usize::MAX,
                ..limits()
            },
        ] {
            assert!(Scheduler::new(invalid, blocking_worker()).is_err());
        }
    }

    #[test]
    fn background_redraw_coalesces_output_and_sleeps_when_clean() {
        use std::time::Duration;
        let mut redraw = DirtyRedraw::new(Duration::ZERO, RedrawLimits::default()).unwrap();
        assert_eq!(redraw.wait(Duration::ZERO), Some(Duration::ZERO));
        redraw.drawn(Duration::ZERO);
        assert_eq!(redraw.wait(Duration::from_secs(3600)), None);
        for _ in 0..63 {
            redraw.dirty(false);
            assert_eq!(
                redraw.wait(Duration::from_millis(5)),
                Some(Duration::from_millis(11))
            );
        }
        assert_eq!(redraw.wait(Duration::from_millis(16)), Some(Duration::ZERO));
        redraw.drawn(Duration::from_millis(16));
        assert_eq!(redraw.wait(Duration::from_secs(3600)), None);
    }

    #[test]
    fn background_redraw_budget_and_explicit_input_prevent_frame_starvation() {
        use std::time::Duration;
        let mut redraw = DirtyRedraw::new(Duration::ZERO, RedrawLimits::default()).unwrap();
        redraw.drawn(Duration::ZERO);
        for _ in 0..64 {
            redraw.dirty(false);
        }
        assert_eq!(redraw.wait(Duration::from_millis(1)), Some(Duration::ZERO));
        redraw.drawn(Duration::from_millis(1));
        redraw.dirty(false);
        assert_eq!(
            redraw.wait(Duration::from_millis(2)),
            Some(Duration::from_millis(15))
        );
        redraw.dirty(true);
        assert_eq!(redraw.wait(Duration::from_millis(2)), Some(Duration::ZERO));
        redraw.drawn(Duration::from_millis(2));
        assert_eq!(redraw.wait(Duration::from_millis(2)), None);
    }

    #[test]
    fn background_redraw_rejects_zero_budget_and_clock_interval() {
        use std::time::Duration;
        assert!(DirtyRedraw::new(
            Duration::ZERO,
            RedrawLimits {
                event_budget: 0,
                ..RedrawLimits::default()
            }
        )
        .is_err());
        assert!(DirtyRedraw::new(
            Duration::ZERO,
            RedrawLimits {
                frame_interval: Duration::ZERO,
                ..RedrawLimits::default()
            }
        )
        .is_err());
    }

    struct PanickingPayload;

    impl Payload for PanickingPayload {
        fn payload_bytes(&self) -> usize {
            panic!("injected output accounting panic");
        }
    }

    #[tokio::test]
    async fn background_output_accounting_panic_reports_failure_without_losing_worker() {
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            Worker::Blocking(Arc::new(|_: Vec<u8>, _| PanickingPayload)),
        )
        .unwrap();
        let generation = scheduler
            .try_submit(RequestDomain(1), vec![1])
            .map_err(|e| e.reason)
            .unwrap();
        let result = scheduler.next_result().await.expect("failure completion");
        assert_eq!(result.generation, generation);
        assert!(matches!(result.result, Err(JobError::Panicked)));
        scheduler.shutdown().await;
    }

    async fn full_results() -> (
        Scheduler<Input, Vec<u8>>,
        RequestGeneration,
        RequestGeneration,
    ) {
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                queued_results: 1,
                ..limits()
            },
            blocking_worker(),
        )
        .unwrap();
        let (entered, mut ready) = mpsc::channel(1);
        let first = submit(
            &mut scheduler,
            1,
            Input {
                entered: Some(entered.clone()),
                ..input(1)
            },
        );
        ready.recv().await.unwrap();
        let second = submit(
            &mut scheduler,
            2,
            Input {
                entered: Some(entered),
                ..input(2)
            },
        );
        // The second job cannot start until the first result is in the queue.
        ready.recv().await.unwrap();
        assert_eq!(scheduler.results.len(), 1);
        (scheduler, first, second)
    }

    #[tokio::test]
    async fn background_result_flood_is_bounded_and_cancellation_wakes_backpressure() {
        let (mut scheduler, first, second) = full_results().await;
        let (entered, mut ready) = mpsc::channel(1);
        submit(
            &mut scheduler,
            3,
            Input {
                entered: Some(entered),
                ..input(3)
            },
        );
        let fourth = submit(&mut scheduler, 4, input(4));
        for _ in 0..1000 {
            let refused = scheduler
                .try_submit(RequestDomain(4), input(5))
                .err()
                .unwrap();
            assert_eq!(refused.reason, AdmissionError::Full);
            assert_eq!(refused.input.bytes, [5]);
            assert!(
                scheduler.accepts(fourth),
                "failed admission must not supersede"
            );
            assert_eq!(scheduler.results.len(), 1);
        }
        assert!(scheduler.cancel(second));
        ready.recv().await.unwrap();
        assert_eq!(scheduler.results.len(), 1);
        assert!(scheduler.cancel(first));
        let three = scheduler.next_result().await.unwrap();
        assert_eq!(three.result, Ok(vec![3]));
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![4]));
        assert!(scheduler.requests.lock().unwrap().current.is_empty());
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_shutdown_wakes_full_result_queue_and_retires_queued_work() {
        let (mut scheduler, first, second) = full_results().await;
        let queued = submit(&mut scheduler, 3, input(3));
        scheduler.shutdown().await;
        assert!(!scheduler.accepts(first));
        assert!(!scheduler.accepts(second));
        assert!(!scheduler.accepts(queued));
        assert!(scheduler.next_result().await.is_none());
        assert!(scheduler.workers.is_empty());
        assert!(scheduler.requests.lock().unwrap().current.is_empty());
    }

    #[tokio::test]
    async fn background_closed_result_receiver_wakes_worker_and_closes_job_admission() {
        let (mut scheduler, _, _) = full_results().await;
        scheduler.results.close();
        for worker in &mut scheduler.workers {
            worker.await.unwrap();
        }
        scheduler.workers.clear();
        let rejected = scheduler
            .try_submit(RequestDomain(3), input(3))
            .err()
            .unwrap();
        assert_eq!(rejected.reason, AdmissionError::Closed);
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_drop_wakes_result_producer_without_ui_thread_join() {
        let (mut scheduler, _, _) = full_results().await;
        let worker = scheduler.workers.pop().unwrap();
        let requests = scheduler.requests.clone();
        drop(scheduler);
        worker.await.unwrap();
        assert!(requests.lock().unwrap().current.is_empty());
    }

    #[tokio::test]
    async fn background_cancelled_queued_job_never_runs_and_retains_other_domain() {
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            blocking_worker(),
        )
        .unwrap();
        let (entered, mut ready) = mpsc::channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
        let running = submit(
            &mut scheduler,
            1,
            Input {
                entered: Some(entered),
                gate: Some(gate),
                ..input(1)
            },
        );
        ready.recv().await.unwrap();
        // Panicking queued job must never execute, even after its slot frees.
        let queued = submit(
            &mut scheduler,
            2,
            Input {
                panic: true,
                ..input(2)
            },
        );
        assert!(scheduler.cancel(queued));
        submit(&mut scheduler, 3, input(3));
        assert!(scheduler.accepts(running));
        release.send(()).unwrap();
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![1]));
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![3]));
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_domain_rejection_releases_reserved_job_slot() {
        let mut scheduler = Scheduler::new(
            Limits {
                request_domains: 1,
                workers: 1,
                ..limits()
            },
            blocking_worker(),
        )
        .unwrap();
        let (entered, mut ready) = mpsc::channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
        let old = submit(
            &mut scheduler,
            1,
            Input {
                entered: Some(entered),
                gate: Some(gate),
                ..input(1)
            },
        );
        ready.recv().await.unwrap();
        let refused = scheduler
            .try_submit(RequestDomain(2), input(2))
            .err()
            .unwrap();
        assert_eq!(refused.reason, AdmissionError::DomainLimit);
        assert!(scheduler.accepts(old));
        let current = submit(&mut scheduler, 1, input(3));
        assert!(!scheduler.accepts(old));
        release.send(()).unwrap();
        assert_eq!(scheduler.next_result().await.unwrap().generation, current);
        submit(&mut scheduler, 2, input(2));
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![2]));
        scheduler.shutdown().await;
    }

    #[test]
    fn background_generation_exhaustion_never_wraps_or_cancels_current_request() {
        let mut registry = Requests::new(1);
        let current = request(&mut registry, RequestDomain(1));
        registry.next = u64::MAX;
        assert_eq!(
            registry.begin(RequestDomain(1)),
            Err(AdmissionError::GenerationExhausted)
        );
        assert!(registry.accepts(current));
        registry.close();
        assert_eq!(
            registry.begin(RequestDomain(1)),
            Err(AdmissionError::Closed)
        );
    }

    #[test]
    fn background_construction_without_runtime_is_a_reported_error() {
        assert!(Scheduler::new(limits(), blocking_worker()).is_err());
    }

    #[tokio::test]
    async fn background_cancellation_notification_survives_before_and_after_registration() {
        use std::task::Poll;
        let already = Cancellation::new();
        already.cancel();
        already.cancelled().await;
        let token = Cancellation::new();
        let notified = token.cancelled();
        tokio::pin!(notified);
        std::future::poll_fn(|cx| {
            assert!(notified.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        token.cancel();
        token.cancel();
        notified.await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn background_async_factory_panic_reports_error_and_worker_continues() {
        let worker = Worker::Async(Arc::new(|bytes: Vec<u8>, _| {
            assert!(bytes != [1], "injected async factory panic");
            Box::pin(async move { bytes }) as Pin<Box<dyn Future<Output = Vec<u8>> + Send>>
        }));
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            worker,
        )
        .unwrap();
        scheduler
            .try_submit(RequestDomain(1), vec![1])
            .map_err(|e| e.reason)
            .unwrap();
        assert_eq!(
            scheduler.next_result().await.unwrap().result,
            Err(JobError::Panicked)
        );
        scheduler
            .try_submit(RequestDomain(2), vec![2])
            .map_err(|e| e.reason)
            .unwrap();
        assert_eq!(scheduler.next_result().await.unwrap().result, Ok(vec![2]));
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn background_async_shutdown_aborts_pending_job_and_wakes_idle_workers() {
        let (entered, mut ready) = mpsc::channel(1);
        let worker = Worker::Async(Arc::new(move |_: Vec<u8>, _| {
            let entered = entered.clone();
            Box::pin(async move {
                entered.send(()).await.unwrap();
                std::future::pending::<Vec<u8>>().await
            }) as Pin<Box<dyn Future<Output = Vec<u8>> + Send>>
        }));
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            worker,
        )
        .unwrap();
        scheduler
            .try_submit(RequestDomain(1), vec![1])
            .map_err(|e| e.reason)
            .unwrap();
        ready.recv().await.unwrap();
        scheduler.shutdown().await;
        assert!(scheduler.next_result().await.is_none());
    }

    #[tokio::test]
    async fn background_cancelled_shutdown_future_keeps_join_handles_for_retry() {
        use std::task::Poll;
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            blocking_worker(),
        )
        .unwrap();
        let (entered, mut ready) = mpsc::channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
        let generation = submit(
            &mut scheduler,
            1,
            Input {
                entered: Some(entered),
                gate: Some(gate),
                ..input(1)
            },
        );
        ready.recv().await.unwrap();
        {
            let shutdown = scheduler.shutdown();
            tokio::pin!(shutdown);
            std::future::poll_fn(|cx| {
                assert!(shutdown.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        assert!(!scheduler.accepts(generation));
        assert_eq!(
            scheduler.workers.len(),
            1,
            "join must not be detached by cancelled shutdown"
        );
        release.send(()).unwrap();
        scheduler.shutdown().await;
        assert!(scheduler.workers.is_empty());
    }

    #[tokio::test]
    async fn background_cooperative_blocking_job_is_woken_by_shutdown() {
        let (entered, mut ready) = mpsc::channel(1);
        let worker = Worker::Blocking(Arc::new(move |_: Vec<u8>, cancel: Cancellation| {
            entered.blocking_send(()).unwrap();
            tokio::runtime::Handle::current().block_on(cancel.cancelled());
            vec![1]
        }));
        let mut scheduler = Scheduler::new(
            Limits {
                workers: 1,
                ..limits()
            },
            worker,
        )
        .unwrap();
        let generation = scheduler
            .try_submit(RequestDomain(1), vec![1])
            .map_err(|e| e.reason)
            .unwrap();
        ready.recv().await.unwrap();
        scheduler.shutdown().await;
        assert!(!scheduler.accepts(generation));
        assert!(scheduler.next_result().await.is_none());
    }

    #[tokio::test]
    async fn background_default_admission_has_a_finite_queue_before_workers_run() {
        // Tokio's current-thread test runtime cannot run workers until yielding.
        let mut scheduler = Scheduler::new(Limits::default(), blocking_worker()).unwrap();
        for domain in 0..16 {
            submit(&mut scheduler, domain, input(domain as u8));
        }
        let rejected = scheduler
            .try_submit(RequestDomain(16), input(16))
            .err()
            .unwrap();
        assert_eq!(rejected.reason, AdmissionError::Full);
        scheduler.shutdown().await;
        assert!(scheduler.next_result().await.is_none());
    }

    #[test]
    fn background_redraw_clock_regression_and_overflow_never_busy_loop() {
        use std::time::Duration;
        let mut redraw = DirtyRedraw::new(Duration::MAX, RedrawLimits::default()).unwrap();
        redraw.drawn(Duration::MAX);
        redraw.dirty(false);
        assert_eq!(redraw.wait(Duration::ZERO), Some(Duration::from_millis(16)));
        redraw.drawn(Duration::ZERO);
        redraw.dirty(false);
        assert_eq!(redraw.wait(Duration::MAX), Some(Duration::from_millis(16)));
        redraw.drawn(Duration::MAX);
        assert_eq!(redraw.wait(Duration::MAX), None);
    }
}
