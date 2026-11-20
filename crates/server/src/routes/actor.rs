//! Who acted, as the Building's admission records name it.

use crate::{admin_token_of, session_of, HeaderMap, Hub};

/// The rank behind a request, written the way admission records name their
/// actor: `master`, or `smaster:<name>`.
///
/// It is read from the same authority every gate uses (`resolve_authority`),
/// so a Smaster signed in with a session is named exactly as one presenting
/// its raw key, and under its current name after a rename. Callers have
/// already required Building rank (`is_admin_req`).
pub(crate) fn rank_actor(hub: &Hub, headers: &HeaderMap) -> String {
    hub.resolve_authority(admin_token_of(headers), session_of(headers))
        .invitation_issuer()
}
