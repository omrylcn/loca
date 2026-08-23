use rusqlite::{params, OptionalExtension, Row, Transaction};

use super::{parse_sender_type, BuildingRole, LocaOperatorAssignment, LocaOperatorError, Store};

impl Store {
    /// Return the room's one active explicit Loca Operator, if appointed.
    pub fn loca_operator(&self, room: &str) -> Option<LocaOperatorAssignment> {
        let c = self.conn()?;
        c.query_row(
            &operator_query("a.room = ?1 AND a.revoked_at IS NULL"),
            params![room],
            assignment_from_row,
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Full assignment history for audit/review, oldest first.
    pub fn loca_operator_history(&self, room: &str) -> Vec<LocaOperatorAssignment> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut stmt) = c.prepare(&format!(
            "{} ORDER BY a.appointed_at, a.id",
            operator_query("a.room = ?1")
        )) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![room], assignment_from_row) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }

    /// Atomically appoint or replace the room's explicit operator. Repeating
    /// the same appointment is idempotent and does not manufacture history.
    pub fn appoint_loca_operator(
        &self,
        room: &str,
        principal_id: &str,
        appointed_by_principal_id: &str,
        appointed_at: u64,
    ) -> Result<LocaOperatorAssignment, LocaOperatorError> {
        let Some(mut c) = self.conn() else {
            return Err(LocaOperatorError::Storage);
        };
        let tx = c.transaction().map_err(storage)?;
        validate_target(&tx, principal_id)?;
        let appointed_by_role = validate_appointer(&tx, appointed_by_principal_id)?;
        if appointed_by_role == BuildingRole::Member {
            return Err(LocaOperatorError::AppointerNotAuthorized);
        }

        let current = query_active(&tx, room)?;
        if let Some(current) = current {
            if current.principal_id == principal_id {
                tx.commit().map_err(storage)?;
                return Ok(current);
            }
            // This check and the active-seat lookup are deliberately inside
            // the same transaction. Two concurrent Smaster requests may both
            // observe an empty seat before entering Store; only the first may
            // fill it and the second must fail without revoking its work.
            if appointed_by_role == BuildingRole::Smaster {
                return Err(LocaOperatorError::EmptySeatRequired);
            }
            tx.execute(
                "UPDATE room_operator_assignments SET revoked_at = ?2
                 WHERE room = ?1 AND revoked_at IS NULL",
                params![room, appointed_at],
            )
            .map_err(storage)?;
        }

        tx.execute(
            "INSERT INTO room_operator_assignments
             (room, principal_id, appointed_by_principal_id, appointed_by_role,
              appointed_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                room,
                principal_id,
                appointed_by_principal_id,
                role_str(appointed_by_role),
                appointed_at
            ],
        )
        .map_err(storage)?;
        let assignment = query_active(&tx, room)?.ok_or(LocaOperatorError::Storage)?;
        tx.commit().map_err(storage)?;
        Ok(assignment)
    }

    /// Revoke using compare-and-swap identity. Callers can authorize against
    /// `loca_operator()` metadata; a concurrent replacement fails closed.
    pub fn revoke_loca_operator(
        &self,
        room: &str,
        expected_principal_id: &str,
        revoked_at: u64,
    ) -> Result<LocaOperatorAssignment, LocaOperatorError> {
        let Some(mut c) = self.conn() else {
            return Err(LocaOperatorError::Storage);
        };
        let tx = c.transaction().map_err(storage)?;
        let mut assignment = query_active(&tx, room)?.ok_or(LocaOperatorError::NotFound)?;
        if assignment.principal_id != expected_principal_id {
            return Err(LocaOperatorError::Conflict);
        }
        let changed = tx
            .execute(
                "UPDATE room_operator_assignments SET revoked_at = ?3
                 WHERE room = ?1 AND principal_id = ?2 AND revoked_at IS NULL",
                params![room, expected_principal_id, revoked_at],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(LocaOperatorError::Conflict);
        }
        assignment.revoked_at = Some(revoked_at);
        tx.commit().map_err(storage)?;
        Ok(assignment)
    }
}

fn validate_target(tx: &Transaction<'_>, principal_id: &str) -> Result<(), LocaOperatorError> {
    let kind: Option<String> = tx
        .query_row(
            "SELECT kind FROM principals WHERE id = ?1 AND disabled_at IS NULL",
            params![principal_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    match kind.as_deref() {
        None => Err(LocaOperatorError::PrincipalNotFound),
        Some("human") => Ok(()),
        Some(_) => Err(LocaOperatorError::PrincipalMustBeHuman),
    }
}

fn validate_appointer(
    tx: &Transaction<'_>,
    principal_id: &str,
) -> Result<BuildingRole, LocaOperatorError> {
    let role: Option<String> = tx
        .query_row(
            "SELECT building_role FROM principals WHERE id = ?1 AND disabled_at IS NULL",
            params![principal_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    role.as_deref()
        .map(parse_role)
        .ok_or(LocaOperatorError::AppointerNotFound)
}

fn query_active(
    tx: &Transaction<'_>,
    room: &str,
) -> Result<Option<LocaOperatorAssignment>, LocaOperatorError> {
    tx.query_row(
        &operator_query("a.room = ?1 AND a.revoked_at IS NULL"),
        params![room],
        assignment_from_row,
    )
    .optional()
    .map_err(storage)
}

fn operator_query(predicate: &str) -> String {
    format!(
        "SELECT a.room, a.principal_id, p.display_name, p.kind,
                a.appointed_by_principal_id, a.appointed_by_role,
                a.appointed_at, a.revoked_at
         FROM room_operator_assignments a
         JOIN principals p ON p.id = a.principal_id
         WHERE {predicate}"
    )
}

fn assignment_from_row(row: &Row<'_>) -> rusqlite::Result<LocaOperatorAssignment> {
    let kind: String = row.get(3)?;
    let role: String = row.get(5)?;
    Ok(LocaOperatorAssignment {
        room: row.get(0)?,
        principal_id: row.get(1)?,
        display_name: row.get(2)?,
        kind: parse_sender_type(&kind),
        appointed_by_principal_id: row.get(4)?,
        appointed_by_role: parse_role(&role),
        appointed_at: row.get(6)?,
        revoked_at: row.get(7)?,
    })
}

fn role_str(role: BuildingRole) -> &'static str {
    match role {
        BuildingRole::Master => "master",
        BuildingRole::Smaster => "smaster",
        BuildingRole::Member => "member",
    }
}

fn parse_role(role: &str) -> BuildingRole {
    match role {
        "master" => BuildingRole::Master,
        "smaster" => BuildingRole::Smaster,
        _ => BuildingRole::Member,
    }
}

fn storage(_: rusqlite::Error) -> LocaOperatorError {
    LocaOperatorError::Storage
}
