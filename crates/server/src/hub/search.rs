//! Building-wide search: the words of every loca the caller may enter, in
//! one list, newest first.
//!
//! It adds no door of its own. The caller hands in `reach`, the same door
//! decision `GET /rooms` lists with (`Hub::enter_decision`), and only the
//! locas it lets in are read at all: an invite's one loca, a direct message
//! only for its two people, Care only for the ranks and its caretaker. The
//! query itself is the per-loca search (`Store::search_messages`) run over
//! that set.

use protocol::SenderType;
use serde::Serialize;

use super::Hub;
use crate::sync::RecoverMutex;

/// The longest snippet of a message's text a hit carries, in characters.
pub const SNIPPET_CHARS: usize = 240;
/// How much text a snippet keeps before the match, in characters.
const SNIPPET_LEAD: usize = 60;

/// One message found by the Building search.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub room: String,
    pub id: u64,
    pub sender: String,
    pub sender_type: SenderType,
    pub ts: u64,
    /// A bounded snippet of the text around the first match.
    pub text: String,
}

impl Hub {
    /// Messages matching `q` in every loca `reach` lets the caller enter,
    /// newest first, at most `limit`. A sealed (deleted) loca is never read.
    pub fn search_building(
        &self,
        q: &str,
        limit: usize,
        reach: impl Fn(&str) -> bool,
    ) -> rusqlite::Result<Vec<SearchHit>> {
        // Decide reachability BEFORE taking the rooms lock: `reach` calls
        // `enter_decision`, which locks `rooms` itself (see
        // `room_summaries_for`).
        let names: Vec<String> = {
            let rooms = self.rooms.lock_or_recover();
            rooms.keys().cloned().collect()
        };
        let open: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|room| !self.is_deleted(room) && reach(room))
            .collect();
        let messages = self.store.search_messages_in(&open, q, limit)?;
        Ok(messages
            .into_iter()
            .map(|m| SearchHit {
                text: snippet(&m.text, q),
                room: m.room,
                id: m.id,
                sender: m.sender,
                sender_type: m.sender_type,
                ts: m.ts,
            })
            .collect())
    }
}

/// At most [`SNIPPET_CHARS`] characters of `text`, around the first match of
/// `q` (folded the way the search folds), with `…` where text was cut.
pub fn snippet(text: &str, q: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= SNIPPET_CHARS {
        return text.to_string();
    }
    // Fold char by char, remembering which character each folded byte came
    // from, so a match in the folded text points back into the original.
    let mut folded = String::new();
    let mut origin = Vec::new();
    for (index, ch) in chars.iter().enumerate() {
        let f = crate::store::search_fold(&ch.to_string());
        origin.extend(std::iter::repeat_n(index, f.len()));
        folded.push_str(&f);
    }
    let needle = crate::store::search_fold(q);
    let at = if needle.is_empty() {
        0
    } else {
        folded
            .find(&needle)
            .and_then(|byte| origin.get(byte).copied())
            .unwrap_or(0)
    };
    let end = (at.saturating_sub(SNIPPET_LEAD) + SNIPPET_CHARS).min(chars.len());
    let start = end.saturating_sub(SNIPPET_CHARS);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_text_is_whole() {
        assert_eq!(snippet("the refresh token", "token"), "the refresh token");
    }

    #[test]
    fn a_long_text_is_cut_around_the_match() {
        let text = format!("{}NEEDLE{}", "a".repeat(500), "b".repeat(500));
        let cut = snippet(&text, "needle");
        assert!(cut.starts_with('…') && cut.ends_with('…'));
        assert!(cut.contains("NEEDLE"));
        assert!(cut.chars().count() <= SNIPPET_CHARS + 2);
    }

    #[test]
    fn a_match_near_the_end_keeps_the_full_window() {
        let text = format!("{}İstanbul", "x".repeat(600));
        let cut = snippet(&text, "istanbul");
        assert!(cut.ends_with("İstanbul"));
        assert_eq!(cut.chars().count(), SNIPPET_CHARS + 1);
    }

    #[test]
    fn no_match_in_the_text_shows_its_start() {
        let text = "z".repeat(400);
        let cut = snippet(&text, "sender-name");
        assert!(!cut.starts_with('…') && cut.ends_with('…'));
    }
}
