//! The building's REST surface, as the master sees it.
//!
//! Every call carries the master key. Nothing here is reachable without it —
//! that is the point: issuing and ending davets is the master's authority and
//! no one else's.

use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Invite {
    pub id: String,
    pub room: String,
    pub name: String,
    pub kind: String,
}

#[derive(Deserialize)]
struct RoomRow {
    room: String,
}

#[derive(Deserialize)]
struct IssuedDavet {
    token: String,
}

pub struct Api {
    server: String,
    key: String,
}

type R<T> = Result<T, String>;

impl Api {
    pub fn new(server: String, key: String) -> Self {
        Api {
            server: server.trim_end_matches('/').to_string(),
            key,
        }
    }

    fn get(&self, path: &str) -> R<ureq::Response> {
        ureq::get(&format!("{}{path}", self.server))
            .set("x-admin-token", &self.key)
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(explain)
    }

    /// Every loca in the building.
    pub fn locas(&self) -> R<Vec<String>> {
        let rows: Vec<RoomRow> = self.get("/rooms")?.into_json().map_err(|e| e.to_string())?;
        Ok(rows.into_iter().map(|r| r.room).collect())
    }

    /// Who holds a davet for this loca.
    pub fn invites(&self, loca: &str) -> R<Vec<Invite>> {
        self.get(&format!("/rooms/{loca}/invites"))?
            .into_json()
            .map_err(|e| e.to_string())
    }

    /// Mint one. Returns the davet itself — shown once, handed over by eye.
    pub fn issue(&self, loca: &str, name: &str, kind: &str) -> R<String> {
        let resp = ureq::post(&format!("{}/rooms/{loca}/invites", self.server))
            .set("x-admin-token", &self.key)
            .timeout(std::time::Duration::from_secs(10))
            .send_json(ureq::json!({ "name": name, "kind": kind }))
            .map_err(explain)?;
        let d: IssuedDavet = resp.into_json().map_err(|e| e.to_string())?;
        Ok(d.token)
    }

    /// End one. The row survives revocation, so "who was let in" stays
    /// answerable afterwards.
    pub fn revoke(&self, loca: &str, id: &str) -> R<()> {
        ureq::delete(&format!("{}/rooms/{loca}/invites/{id}", self.server))
            .set("x-admin-token", &self.key)
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map(|_| ())
            .map_err(explain)
    }
}

/// Turn transport failures into something a person can act on.
fn explain(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => "the master key was refused".into(),
        ureq::Error::Status(404, _) => "no such loca".into(),
        ureq::Error::Status(c, _) => format!("the building answered {c}"),
        ureq::Error::Transport(t) => format!("cannot reach the building ({t})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The console's whole job, against a live building: see the locas, mint a
    /// davet, see it listed, take it back.
    #[test]
    fn issue_list_revoke_round_trip() {
        let Some(server) = std::env::var("LOCA_TEST_SERVER").ok() else {
            eprintln!("skipped: set LOCA_TEST_SERVER to run");
            return;
        };
        let key = std::env::var("LOCA_TEST_KEY").unwrap_or_default();
        let api = Api::new(server, key);

        let locas = api.locas().expect("locas");
        assert!(locas.iter().any(|l| l == "general"), "general must exist");

        let token = api
            .issue("general", "console-test", "agent")
            .expect("issue");
        assert!(token.starts_with("dv_"), "a davet is minted, got {token}");

        let held = api.invites("general").expect("list");
        let id = held
            .iter()
            .find(|i| i.name == "console-test")
            .map(|i| i.id.clone())
            .expect("the davet shows up under its holder");
        assert!(id.starts_with("ivid_"), "a safe management id is listed");
        assert_ne!(id, token, "the one-time secret must not be listed");

        api.revoke("general", &id).expect("revoke");
        let after = api.invites("general").expect("list again");
        assert!(
            !after.iter().any(|i| i.id == id),
            "a revoked davet leaves the list"
        );
    }
}
