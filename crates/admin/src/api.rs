//! The building's REST surface, as the master sees it.
//!
//! Every call carries the master key. Nothing here is reachable without it —
//! that is the point: issuing and ending invites is the master's authority and
//! no one else's.

use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Invite {
    pub id: String,
    pub room: String,
    pub name: String,
    pub kind: String,
}

/// `GET /rooms` says nothing of a room's kind, so a direct message is told by the
/// id prefix the server reserves for direct messages and refuses to any other loca
/// (`Hub::DM_PREFIX`; the web client uses the same one).
const DM_PREFIX: &str = "bb-";

#[derive(Deserialize)]
struct RoomRow {
    room: String,
}

#[derive(Deserialize)]
struct IssuedInvite {
    token: String,
}

/// What a re-send answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Reissued {
    /// The fresh invite, shown once like a minted one.
    Fresh(String),
    /// An older building ignored the request and answered as to a call:
    /// nothing was re-sent and the invite is unchanged.
    Unsupported,
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

    /// The master key, a ten-second limit, and every status answered as a
    /// response: `answered` reads a refusal's own sentence.
    fn keyed<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        request
            .header("x-admin-token", self.key.as_str())
            .config()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .http_status_as_error(false)
            .build()
    }

    fn get(&self, path: &str) -> R<Reply> {
        answered(
            self.keyed(ureq::get(&format!("{}{path}", self.server)))
                .call(),
            false,
        )
    }

    /// Every loca in the building whose invites the master manages. A
    /// direct message is conversation only (PRINCIPLES "Birebir"): nobody issues or
    /// reads invites for it, so it is not listed.
    pub fn locas(&self) -> R<Vec<String>> {
        let rows: Vec<RoomRow> = json(self.get("/rooms")?)?;
        Ok(rows
            .into_iter()
            .map(|r| r.room)
            .filter(|room| !room.starts_with(DM_PREFIX))
            .collect())
    }

    /// Who holds an invite for this loca.
    pub fn invites(&self, loca: &str) -> R<Vec<Invite>> {
        json(self.get(&format!("/rooms/{loca}/invites"))?)
    }

    /// Mint one. Returns the invite itself — shown once, handed over by eye.
    pub fn issue(&self, loca: &str, name: &str) -> R<String> {
        let resp = answered(
            self.keyed(ureq::post(&format!("{}/rooms/{loca}/invites", self.server)))
                .send_json(serde_json::json!({ "name": name })),
            true,
        )?;
        let d: IssuedInvite = json(resp)?;
        Ok(d.token)
    }

    /// Re-send one: the holder's invite for this loca ends and a fresh one
    /// takes its place, delivered over their Lobby line like a call. The seat
    /// stays theirs.
    pub fn reissue(&self, loca: &str, name: &str) -> R<Reissued> {
        let resp = answered(
            self.keyed(ureq::post(&format!("{}/rooms/{loca}/call", self.server)))
                .send_json(serde_json::json!({ "name": name, "reissue": true })),
            true,
        )?;
        let answer: serde_json::Value = json(resp)?;
        reissued_from(&answer)
    }

    /// End one. The row survives revocation, so "who was let in" stays
    /// answerable afterwards.
    pub fn revoke(&self, loca: &str, id: &str) -> R<()> {
        answered(
            self.keyed(ureq::delete(&format!(
                "{}/rooms/{loca}/invites/{id}",
                self.server
            )))
            .call(),
            false,
        )
        .map(|_| ())
    }
}

type Reply = ureq::http::Response<ureq::Body>;

fn json<T: serde::de::DeserializeOwned>(mut reply: Reply) -> R<T> {
    reply.body_mut().read_json().map_err(|e| e.to_string())
}

fn reissued_from(answer: &serde_json::Value) -> R<Reissued> {
    if answer.get("already").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(Reissued::Unsupported);
    }
    answer
        .get("token")
        .and_then(|v| v.as_str())
        .map(|token| Reissued::Fresh(token.to_string()))
        .ok_or_else(|| "the building answered without an invite".into())
}

/// Turn an answer into something a person can act on. A refused key and an
/// unreachable building say so; with `keep_reason`, any other refusal keeps
/// the building's own sentence, since an issue or a re-send can be refused
/// for several reasons a bare status cannot tell apart (not a member yet, a
/// direct message, a full loca).
fn answered(result: Result<Reply, ureq::Error>, keep_reason: bool) -> R<Reply> {
    let mut reply = result.map_err(|e| format!("cannot reach the building ({e})"))?;
    let status = reply.status().as_u16();
    if status < 400 {
        return Ok(reply);
    }
    if status == 401 {
        return Err("the master key was refused".into());
    }
    if keep_reason {
        let reason = reply.body_mut().read_to_string().unwrap_or_default();
        if !reason.trim().is_empty() {
            return Err(reason.trim().to_string());
        }
    } else if status == 404 {
        return Err("no such loca".into());
    }
    Err(format!("the building answered {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh invite comes back as one; an older building that ignores the
    /// flag answers `already`, which is not mistaken for a re-send.
    #[test]
    fn a_reissue_answer_is_read_for_what_it_is() {
        assert_eq!(
            reissued_from(&serde_json::json!({ "name": "w", "loca": "l", "token": "dv_x" })),
            Ok(Reissued::Fresh("dv_x".into()))
        );
        assert_eq!(
            reissued_from(&serde_json::json!({ "already": true, "name": "w" })),
            Ok(Reissued::Unsupported)
        );
        assert!(reissued_from(&serde_json::json!({ "name": "w" })).is_err());
    }

    /// A building that answers every request with one refusal, after reading
    /// the whole request (body included, so the client is never cut off).
    fn refusing_building(status: u16, reason: &'static str) -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            length = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: text/plain\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{reason}",
                    reason.len()
                );
            }
        });
        format!("http://{addr}")
    }

    /// Issuing can be refused for reasons only the building can name; the
    /// console shows its sentence, not a bare status code.
    #[test]
    fn an_issue_refusal_keeps_the_buildings_reason() {
        let api = Api::new(
            refusing_building(403, "a direct message is conversation only"),
            "synthetic-master-key".into(),
        );
        assert_eq!(
            api.issue("bb-5aa44a1ddc8f0e107ce8bbdcd6cc327f", "ayse"),
            Err("a direct message is conversation only".to_string())
        );
        let api = Api::new(
            refusing_building(409, "this agent does not belong to the building yet"),
            "synthetic-master-key".into(),
        );
        assert_eq!(
            api.issue("alpha", "nobody"),
            Err("this agent does not belong to the building yet".to_string())
        );
    }

    /// The console's whole job, against a live building: see the locas, mint an
    /// invite, see it listed, take it back.
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

        let token = api.issue("general", "console-test").expect("issue");
        assert!(token.starts_with("dv_"), "an invite is minted, got {token}");

        let held = api.invites("general").expect("list");
        let id = held
            .iter()
            .find(|i| i.name == "console-test")
            .map(|i| i.id.clone())
            .expect("the invite shows up under its holder");
        assert!(id.starts_with("ivid_"), "a safe management id is listed");
        assert_ne!(id, token, "the one-time secret must not be listed");

        // Re-send it: a fresh invite, the old one gone, still one under the name.
        let Reissued::Fresh(fresh) = api.reissue("general", "console-test").expect("re-send")
        else {
            panic!("a current building re-sends");
        };
        assert!(fresh.starts_with("dv_") && fresh != token);
        let held = api.invites("general").expect("list after re-send");
        let ours: Vec<&Invite> = held.iter().filter(|i| i.name == "console-test").collect();
        assert_eq!(ours.len(), 1, "a re-send never leaves two invites");
        assert_ne!(ours[0].id, id, "the re-sent invite is a new one");
        let id = ours[0].id.clone();

        api.revoke("general", &id).expect("revoke");
        let after = api.invites("general").expect("list again");
        assert!(
            !after.iter().any(|i| i.id == id),
            "a revoked invite leaves the list"
        );
    }
}
