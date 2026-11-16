//! The Lead's task authority: declare a task and assign one in its own loca,
//! within the bounds the Operator keeps (PRINCIPLES "Loca layer", Lead).
//!
//! The Lead is recognised by the principal stored with the title
//! (`RoomSettings::lead_principal`), never by the display name. Where no
//! principal exists (the open sandbox, a memory-only server) the name is what
//! there is, exactly as `Hub::is_loca_operator` does for operators there.

use protocol::{Task, TaskActor, TaskStatus};

use super::super::Hub;
use crate::sync::RecoverMutex;

impl Hub {
    /// Whether the caller holds this loca's Lead title, for task authority.
    /// `by` is the name the server verified for the caller and
    /// `by_principal` the principal behind it.
    pub fn is_task_lead(&self, room: &str, by: &str, by_principal: Option<&str>) -> bool {
        let rooms = self.rooms.lock_or_recover();
        let Some(settings) = rooms.get(room).map(|r| &r.settings) else {
            return false;
        };
        match settings.lead_principal.as_deref() {
            Some(lead) => by_principal == Some(lead),
            None => self.names_stand_for_principals() && settings.lead.as_deref() == Some(by),
        }
    }

    /// True where nothing is bound to a principal, so a name is all there
    /// is: no Building key (the open sandbox), or no database.
    pub(super) fn names_stand_for_principals(&self) -> bool {
        self.admin_token.is_empty() || !self.store.is_persistent()
    }

    /// Whether the Lead may move this task's assignment: an unfinished task
    /// that nobody holds, or one whose holder was named by a Lead or claimed
    /// by its holder. An assignment the Operator or a rank made, or one from
    /// before assignments recorded their authority, is not the Lead's.
    pub(super) fn lead_may_assign(task: &Task) -> bool {
        matches!(task.status, TaskStatus::Open | TaskStatus::Taken)
            && (task.assigned_to.is_none()
                || matches!(
                    task.assigned_as,
                    Some(TaskActor::Lead | TaskActor::Participant)
                ))
    }

    /// Whether the Lead asking may cancel this task: one it declared itself,
    /// under its own principal, that nobody has taken yet.
    pub(super) fn lead_may_cancel(
        &self,
        task: &Task,
        by: &str,
        by_principal: Option<&str>,
    ) -> bool {
        task.status == TaskStatus::Open
            && task.created_as == Some(TaskActor::Lead)
            && match task.created_principal.as_deref() {
                Some(declarer) => by_principal == Some(declarer),
                None => self.names_stand_for_principals() && task.created_by == by,
            }
    }
}
