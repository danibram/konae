pub mod budget;
mod containment;
pub mod protocol;
pub mod sandbox;
mod supervisor;

pub use budget::{Budget, BudgetError, MutationOutcomeState};
pub use containment::MemoryPolicy;
pub use protocol::{Input, Output, WorkerLimits};
use std::{sync::Arc, time::Instant};
pub use supervisor::{
    Worker, WorkerError, error_code, error_detail, validate_linux_worker_containment,
    validate_linux_worker_identity,
};

pub struct RequestBodyChunk {
    pub bytes: Vec<u8>,
    pub done: bool,
    _retained: Option<Box<dyn Send>>,
}
impl RequestBodyChunk {
    pub fn new(bytes: Vec<u8>, done: bool) -> Self {
        Self {
            bytes,
            done,
            _retained: None,
        }
    }
    pub fn retaining(bytes: Vec<u8>, done: bool, retained: Box<dyn Send>) -> Self {
        Self {
            bytes,
            done,
            _retained: Some(retained),
        }
    }
}

pub trait RequestBody: Send + Sync {
    fn read_chunk(&self, maximum: usize) -> Result<RequestBodyChunk, String>;
    fn complete(&self) -> bool;
}

pub trait ExecutionBudget {
    fn deadline(&self) -> Instant;
    fn is_cancelled(&self) -> bool;
    fn register_abort_handler(&self, _handler: Arc<dyn Fn() + Send + Sync>) {}
    fn clear_abort_handler(&self) {}
}
