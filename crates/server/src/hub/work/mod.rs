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
    /// The review state was asked for while the Loca Board is off.
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
    /// A goal closed by its tasks set up while the Loca Board, which serves
    /// the tasks, is off: they could not move, so it could not close.
    BoardOff,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    NotFound,
    SelfWait,
    Storage,
}
