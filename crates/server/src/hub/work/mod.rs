mod board;
pub(in crate::hub) use board::settle_board;
#[cfg(test)]
mod board_tests;
mod goal;
mod lead;
mod policy;
mod task;
mod wait;

/// Why a task update was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskError {
    NotFound,
    /// The house rules: agents take/finish their own; operators do the rest.
    Forbidden,
    /// The review state was asked for while the Task board is off.
    BoardOff,
    /// A no-go without a reason.
    ReasonRequired,
    /// A review note or reason over `TASK_NOTE_MAX` characters.
    TooLong,
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
