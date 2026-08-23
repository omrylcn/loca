mod goal;
mod policy;
mod task;
mod wait;

/// Why a task update was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskError {
    NotFound,
    /// The house rules: agents take/finish their own; operators do the rest.
    Forbidden,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalError {
    NotFound,
    ActiveExists,
    LeadRequired,
    InvalidTasks,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    NotFound,
    SelfWait,
    Storage,
}
