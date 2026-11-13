//! HTTP ownership boundary.
//!
//! `main.rs` composes the router and shared state; these modules own request
//! extraction, authorization decisions, and domain-specific HTTP handlers.

mod access;
mod attachments;
mod attention;
mod birebir;
mod content;
mod downloads;
mod extensions;
mod groups;
mod keys;
mod lobby;
mod membership;
mod notifications;
mod rooms;
mod wiki;
mod work;
pub(crate) use wiki::{
    claim_wiki_review, commit_wiki, configure_wiki, edit_wiki_page, finish_wiki_review, get_wiki,
    get_wiki_page, remove_wiki_page, renew_wiki_review, wiki_context, wiki_context_accept,
    wiki_context_offer, wiki_context_request, wiki_context_requests, wiki_maintenance_status,
    wiki_native_ready,
};

#[cfg(test)]
pub(crate) use access::mutation_actor;

pub(crate) use access::{
    admin_token_of, is_admin_req, is_master_req, member_token_of, pairing_code_of,
    require_membership, session_of, valid_identity_name, RoomAccess,
};
pub(crate) use attachments::{get_attachment, upload_attachment};
pub(crate) use attention::{
    ack_care, claim_attention, create_attention, list_attentions, report_runtime_health,
    resolve_attention,
};
pub(crate) use birebir::{
    birebir_candidates_route, birebir_close_route, birebir_fence, birebir_master_route,
    birebir_overview_route, birebir_seal_route, birebir_witness_route, list_my_birebirs,
    open_birebir_route,
};
pub(crate) use content::{
    append_long_memory, caretaker_memory, create_note, delete_note, get_journal, get_loca_memory,
    get_loca_memory_snapshot, get_note, get_notes, get_reactions, list_long_memory_entries,
    note_history, post_journal, post_message, search_room, set_loca_memory_owner, set_reaction,
    update_note, write_short_memory,
};
pub(crate) use downloads::{download_skill_bundle, skill_bundle_manifest, skill_bundles_index};
pub(crate) use extensions::{
    add_extension, extension_catalog, extension_fence, extension_off, get_extensions,
    put_extensions, remove_extension,
};
pub(crate) use groups::{
    add_group_member_route, create_group_route, delete_group_route, get_group, group_fence,
    group_log_route, list_groups, remove_group_member_route, set_loca_group_route,
    update_group_route,
};
pub(crate) use keys::{issue_key, list_keys, rename_smaster_route, revoke_key_route};
pub(crate) use lobby::{call_into_loca, lobby_ws_handler, release_self_from_loca};
pub(crate) use membership::{
    ack_join_request_route, admit_member, approve_join_request_route, bootstrap_join_request_route,
    caretaker_residents, caretaker_viewer, claim_membership, create_admission_stock_route,
    create_join_request_route, create_pairing_route, create_profile_credential_route,
    create_session_route, create_smaster, delete_session_route, deny_join_request_route,
    get_admission_stock_route, get_join_request_route, get_profile_preferences,
    list_building_ranks, list_join_requests_route, list_members, list_profile_credentials,
    list_profiles, list_residents, list_smasters, pairing_ttl_ms, profile_view,
    put_profile_preferences, rename_profile_route, revoke_member_route,
    revoke_profile_credential_route, revoke_smaster_route, set_member_suspension_route, whoami,
};
pub(crate) use notifications::{
    list_notifications, notification_audiences_route, read_all_notifications_route,
    read_notification_route, send_notification_route, sent_notifications_route,
};
pub(crate) use rooms::{
    actor_of, appoint_loca_operator_route, create_invite, create_loca_route, delete_room,
    get_loca_operator, get_members, get_messages, get_mod, get_mode, get_settings,
    list_building_bans, list_invites, list_rooms, moderate, revoke_invite,
    revoke_loca_operator_route, room_decision_of, set_lead, set_mode, set_settings,
};
pub(crate) use work::{
    clear_wait, create_goal, create_task, list_goals, list_tasks, list_waits, set_wait,
    update_goal, update_task,
};
