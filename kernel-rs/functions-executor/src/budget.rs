use std::{
    fmt,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    DeadlineExceeded,
    Cancelled,
}
impl BudgetError {
    pub fn code(self) -> &'static str {
        match self {
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Cancelled => "cancelled",
        }
    }
}
impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for BudgetError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOutcomeState {
    Clean,
    Committing,
    Committed,
    Uncertain,
}
impl MutationOutcomeState {
    pub fn may_have_committed(self) -> bool {
        self != Self::Clean
    }
}

struct Shared {
    deadline: Instant,
    cancelled: AtomicBool,
    abort_requested: AtomicBool,
    mutation: AtomicU8,
    abort: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    wait: Mutex<()>,
    wake: Condvar,
}
#[derive(Clone)]
pub struct Budget {
    shared: Arc<Shared>,
}
impl Budget {
    pub fn new(duration: Duration) -> Self {
        Self::until(Instant::now() + duration)
    }
    pub fn until(deadline: Instant) -> Self {
        Self {
            shared: Arc::new(Shared {
                deadline,
                cancelled: AtomicBool::new(false),
                abort_requested: AtomicBool::new(false),
                mutation: AtomicU8::new(0),
                abort: Mutex::new(None),
                wait: Mutex::new(()),
                wake: Condvar::new(),
            }),
        }
    }
    pub fn deadline(&self) -> Instant {
        self.shared.deadline
    }
    pub fn check(&self) -> Result<(), BudgetError> {
        if Instant::now() >= self.shared.deadline {
            return Err(BudgetError::DeadlineExceeded);
        }
        if self.shared.cancelled.load(Ordering::SeqCst) {
            return Err(BudgetError::Cancelled);
        }
        Ok(())
    }
    pub fn remaining(&self) -> Result<Duration, BudgetError> {
        self.check()?;
        let left = self
            .shared
            .deadline
            .saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(BudgetError::DeadlineExceeded)
        } else {
            Ok(left)
        }
    }
    pub fn cancel(&self) -> MutationOutcomeState {
        let _guard = self.shared.wait.lock().unwrap();
        self.shared.cancelled.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
        self.mutation_outcome()
    }
    pub fn abort(&self) -> MutationOutcomeState {
        let handler = {
            let mut abort = self.shared.abort.lock().unwrap();
            self.shared.abort_requested.store(true, Ordering::SeqCst);
            abort.take()
        };
        let outcome = self.cancel();
        if let Some(handler) = handler {
            handler();
        }
        outcome
    }
    pub fn register_abort_handler(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        let invoke_now = {
            let mut abort = self.shared.abort.lock().unwrap();
            if self.shared.abort_requested.load(Ordering::SeqCst) {
                true
            } else {
                *abort = Some(handler.clone());
                false
            }
        };
        if invoke_now {
            handler();
        }
    }
    pub fn mutation_outcome(&self) -> MutationOutcomeState {
        match self.shared.mutation.load(Ordering::SeqCst) {
            0 => MutationOutcomeState::Clean,
            1 => MutationOutcomeState::Committing,
            2 => MutationOutcomeState::Committed,
            _ => MutationOutcomeState::Uncertain,
        }
    }
    pub fn failure_code(&self) -> Option<&'static str> {
        self.check().err().map(|error| {
            if self.mutation_outcome().may_have_committed() {
                "mutation_outcome_unknown"
            } else {
                error.code()
            }
        })
    }
    pub fn wait_retry(&self) -> Result<(), BudgetError> {
        let guard = self.shared.wait.lock().unwrap();
        let wait = self.remaining()?.min(Duration::from_millis(1));
        let _guard = self.shared.wake.wait_timeout(guard, wait).unwrap();
        self.check()
    }
    pub fn begin_commit(&self) -> Result<(), BudgetError> {
        // El supervisor ve la fase antes de decidir si cancelar implica ausencia de efectos.
        let _ = self
            .shared
            .mutation
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
        self.check()
    }
    pub fn committed(&self) {
        self.shared.mutation.store(2, Ordering::SeqCst);
    }
    pub fn rolled_back(&self) {
        // Un rollback posterior no borra un commit anterior de esta petición.
        let _ = self
            .shared
            .mutation
            .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
    pub fn uncertain(&self) {
        self.shared.mutation.store(3, Ordering::SeqCst);
    }
}

impl crate::ExecutionBudget for Budget {
    fn deadline(&self) -> Instant {
        self.deadline()
    }
    fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }
    fn register_abort_handler(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        Budget::register_abort_handler(self, handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn abort_runs_the_registered_handler_once() {
        let budget = Budget::new(Duration::from_secs(1));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        budget.register_abort_handler(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));

        assert_eq!(budget.abort(), MutationOutcomeState::Clean);
        assert_eq!(budget.failure_code(), Some("cancelled"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ordinary_cancellation_does_not_run_the_abort_handler() {
        let budget = Budget::new(Duration::from_secs(1));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        budget.register_abort_handler(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));

        budget.cancel();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn registering_after_abort_runs_the_handler() {
        let budget = Budget::new(Duration::from_secs(1));
        let calls = Arc::new(AtomicUsize::new(0));
        assert_eq!(budget.abort(), MutationOutcomeState::Clean);

        let observed = calls.clone();
        budget.register_abort_handler(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
