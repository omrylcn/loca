//! HTTP ownership boundary.
//!
//! `main.rs` composes the router and shared state; these modules own request
//! extraction, authorization decisions, and domain-specific HTTP handlers.

mod access;
mod attention;
mod content;
mod lobby;
mod membership;
mod rooms;
mod work;

pub(crate) use access::{
    admin_token_of, is_admin_req, is_master_req, member_token_of, pairing_code_of,
    require_membership, session_of, valid_identity_name, RoomAccess,
};
pub(crate) use attention::{
    ack_care, claim_attention, create_attention, list_attentions, report_runtime_health,
    resolve_attention,
};
pub(crate) use content::{
    create_note, delete_note, get_journal, get_note, get_notes, note_history, post_journal,
    post_message, search_room, update_note,
};
pub(crate) use lobby::{call_into_loca, lobby_ws_handler, release_self_from_loca};
pub(crate) use membership::{
    admit_member, caretaker_residents, claim_membership, create_pairing_route,
    create_profile_credential_route, create_session_route, create_smaster, delete_session_route,
    list_members, list_profile_credentials, list_profiles, list_residents, list_smasters,
    pairing_ttl_ms, profile_view, revoke_member_route, revoke_profile_credential_route,
    revoke_smaster_route, whoami,
};
pub(crate) use rooms::{
    actor_of, appoint_loca_operator_route, create_invite, delete_room, get_loca_operator,
    get_members, get_messages, get_mod, get_mode, get_settings, list_invites, list_rooms, moderate,
    revoke_invite, revoke_loca_operator_route, room_decision_of, set_lead, set_mode, set_settings,
};
pub(crate) use work::{
    clear_wait, create_goal, create_task, list_goals, list_tasks, list_waits, set_wait,
    update_goal, update_task,
};
