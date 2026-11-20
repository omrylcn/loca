//! The HTTP router: every public route, and the fences layered over them.

use crate::*;

/// Every public route of the server, with its fences and body limit.
pub(crate) fn router(hub: Hub) -> Router {
    Router::new()
        .route("/", get(index))
        // The principles are served under their file names, so the language
        // links inside them work here as they do in the repository:
        // PRINCIPLES.md is the English text, PRINCIPLES.tr.md the Turkish.
        // /PRINCIPLES.en.md is the address the English text had before it
        // became PRINCIPLES.md; it is kept so old links still open it.
        .route("/PRINCIPLES.md", get(principles_en))
        .route("/PRINCIPLES.tr.md", get(principles_tr))
        .route("/PRINCIPLES.en.md", get(principles_en))
        .route("/downloads/skills", get(skill_bundles_index))
        .route("/downloads/skills/{name}", get(download_skill_bundle))
        .route(
            "/downloads/skills/{name}/manifest",
            get(skill_bundle_manifest),
        )
        .route("/docs/getting-started.md", get(getting_started_doc))
        .route("/assets/{name}", get(web_asset))
        .route("/health", get(health))
        .route(
            "/sessions",
            axum::routing::post(create_session_route).delete(delete_session_route),
        )
        .route("/pairings", axum::routing::post(create_pairing_route))
        .route(
            "/admission-stock",
            get(get_admission_stock_route).post(create_admission_stock_route),
        )
        .route(
            "/join-requests",
            get(list_join_requests_route).post(create_join_request_route),
        )
        .route("/join-requests/{id}", get(get_join_request_route))
        .route(
            "/join-requests/{id}/approve",
            axum::routing::post(approve_join_request_route),
        )
        .route(
            "/join-requests/{id}/deny",
            axum::routing::post(deny_join_request_route),
        )
        .route(
            "/join-requests/{id}/bootstrap",
            axum::routing::post(bootstrap_join_request_route),
        )
        .route(
            "/join-requests/{id}/ack",
            axum::routing::post(ack_join_request_route),
        )
        .route("/whoami", get(whoami))
        .route("/profile", get(profile_view).put(rename_profile_route))
        .route("/building/ranks", get(list_building_ranks))
        .route("/building/bans", get(list_building_bans))
        .route(
            "/building/settings",
            get(get_building_settings).put(put_building_settings),
        )
        .route("/groups", get(list_groups).post(create_group_route))
        .route(
            "/groups/{id}",
            get(get_group)
                .patch(update_group_route)
                .delete(delete_group_route),
        )
        .route("/groups/{id}/log", get(group_log_route))
        .route(
            "/groups/{id}/members/{principal_id}",
            axum::routing::put(add_group_member_route).delete(remove_group_member_route),
        )
        .route(
            "/rooms/{id}/group",
            axum::routing::put(set_loca_group_route),
        )
        .route("/rooms/{id}/group/preview", get(preview_loca_group_route))
        .route(
            "/notifications",
            get(list_notifications).post(send_notification_route),
        )
        .route("/notifications/sent", get(sent_notifications_route))
        .route(
            "/notifications/audiences",
            get(notification_audiences_route),
        )
        .route(
            "/notifications/read-all",
            axum::routing::post(read_all_notifications_route),
        )
        .route(
            "/notifications/{id}/read",
            axum::routing::post(read_notification_route),
        )
        .route(
            "/profile/preferences",
            get(get_profile_preferences).put(put_profile_preferences),
        )
        .route("/profile/card", axum::routing::put(put_profile_card))
        .route(
            "/profile/credentials",
            get(list_profile_credentials).post(create_profile_credential_route),
        )
        .route(
            "/profile/credentials/{credential_id}",
            axum::routing::delete(revoke_profile_credential_route),
        )
        .route("/membership/claim", axum::routing::post(claim_membership))
        .route(
            "/membership/leave",
            axum::routing::post(leave_building_route),
        )
        .route(
            "/membership/claim/ack",
            axum::routing::post(ack_membership_claim_route),
        )
        .route("/lobby/ws", get(lobby_ws_handler))
        .route("/members", get(list_members).post(admit_member))
        .route("/profiles", get(list_profiles))
        .route("/care/residents", get(caretaker_residents))
        .route("/care/memory", get(caretaker_memory))
        .route(
            "/runtime/health",
            axum::routing::post(report_runtime_health),
        )
        .route(
            "/members/{token}",
            axum::routing::delete(revoke_member_route),
        )
        .route(
            "/members/{token}/suspension",
            axum::routing::post(set_member_suspension_route),
        )
        .route("/residents", get(list_residents))
        .route("/rooms/{id}/call", axum::routing::post(call_into_loca))
        .route(
            "/rooms/{id}/release",
            axum::routing::post(release_self_from_loca),
        )
        .route("/smasters", get(list_smasters).post(create_smaster))
        .route(
            "/smasters/{token}",
            axum::routing::delete(revoke_smaster_route).put(rename_smaster_route),
        )
        .route("/keys", get(list_keys).post(issue_key))
        .route("/keys/{id}", axum::routing::delete(revoke_key_route))
        .route("/rooms", get(list_rooms).post(create_loca_route))
        // Building-wide search: only the locas the caller may enter are read.
        .route("/search", get(search_building))
        .route("/rooms/{id}/messages", get(get_messages).post(post_message))
        .route(
            "/rooms/{id}/attachments",
            // The upload body carries a file, so this one route lifts the tiny
            // global JSON body cap to the attachment limit (+ slack for headers).
            axum::routing::post(upload_attachment).layer(DefaultBodyLimit::max(
                hub::attachment_max_bytes() as usize + 4096,
            )),
        )
        .route("/rooms/{id}/attachments/{att_id}", get(get_attachment))
        .route("/rooms/{id}/reactions", get(get_reactions))
        .route(
            "/rooms/{id}/messages/{message_id}/reactions",
            axum::routing::post(set_reaction),
        )
        .route("/rooms/{id}/members", get(get_members))
        .route("/rooms/{id}/typing", axum::routing::post(post_typing))
        .route("/rooms/{id}/mode", get(get_mode).put(set_mode))
        .route("/rooms/{id}/lead", axum::routing::post(set_lead))
        .route(
            "/rooms/{id}/operators",
            get(get_loca_operator)
                .post(appoint_loca_operator_route)
                .delete(revoke_loca_operator_route),
        )
        .route("/rooms/{id}/settings", get(get_settings).put(set_settings))
        .route("/extensions", get(extension_catalog))
        .route(
            "/rooms/{id}/extensions",
            get(get_extensions).put(put_extensions),
        )
        .route(
            "/rooms/{id}/extensions/{ext}",
            axum::routing::put(add_extension).delete(remove_extension),
        )
        .route("/rooms/{id}/moderate", get(get_mod).post(moderate))
        .route("/rooms/{id}", axum::routing::delete(delete_room))
        .route("/rooms/{id}/archive", get(loca_archive))
        .route(
            "/rooms/{id}/records",
            axum::routing::delete(delete_loca_records),
        )
        .route("/rooms/{id}/invites", get(list_invites).post(create_invite))
        .route(
            "/rooms/{id}/invites/{token}",
            axum::routing::delete(revoke_invite),
        )
        .route("/rooms/{id}/notes", get(get_notes).post(create_note))
        .route("/rooms/{id}/vault", get(get_vault).put(put_vault))
        .route("/rooms/{id}/vault/history", get(get_vault_history))
        .route("/rooms/{id}/wiki", get(get_wiki))
        .route(
            "/rooms/{id}/wiki/pages/{slug}",
            get(get_wiki_page)
                .put(edit_wiki_page)
                .delete(remove_wiki_page),
        )
        .route(
            "/rooms/{id}/wiki/config",
            axum::routing::put(configure_wiki),
        )
        .route("/rooms/{id}/wiki/review", axum::routing::post(commit_wiki))
        .route("/rooms/{id}/wiki/maintenance", get(wiki_maintenance_status))
        .route(
            "/rooms/{id}/wiki/context/requests",
            get(wiki_context_requests),
        )
        .route(
            "/rooms/{id}/wiki/runtime",
            axum::routing::post(wiki_native_ready),
        )
        .route(
            "/rooms/{id}/wiki/context",
            get(wiki_context).post(wiki_context_request),
        )
        .route(
            "/rooms/{id}/wiki/context/accept",
            axum::routing::post(wiki_context_accept),
        )
        .route(
            "/rooms/{id}/wiki/context/offer",
            axum::routing::post(wiki_context_offer),
        )
        .route(
            "/rooms/{id}/wiki/maintenance/claim",
            axum::routing::post(claim_wiki_review),
        )
        .route(
            "/rooms/{id}/wiki/maintenance/finish",
            axum::routing::post(finish_wiki_review),
        )
        .route(
            "/rooms/{id}/wiki/maintenance/renew",
            axum::routing::post(renew_wiki_review),
        )
        .route(
            "/rooms/{id}/notes/{key}",
            get(get_note).put(update_note).delete(delete_note),
        )
        .route("/rooms/{id}/notes/{key}/history", get(note_history))
        // Memory is not a note: deliberately no DELETE method is attached.
        .route("/rooms/{id}/memory", get(get_loca_memory))
        .route("/rooms/{id}/memory/snapshot", get(get_loca_memory_snapshot))
        .route(
            "/rooms/{id}/memory/owner",
            axum::routing::put(set_loca_memory_owner),
        )
        .route(
            "/rooms/{id}/memory/short",
            axum::routing::put(write_short_memory),
        )
        .route(
            "/rooms/{id}/memory/entries",
            axum::routing::post(append_long_memory).get(list_long_memory_entries),
        )
        .route("/rooms/{id}/search", get(search_room))
        .route("/rooms/{id}/journal", get(get_journal).post(post_journal))
        .route("/rooms/{id}/tasks", get(list_tasks).post(create_task))
        .route("/rooms/{id}/tasks/{tid}", get(get_task).patch(update_task))
        .route("/rooms/{id}/goals", get(list_goals).post(create_goal))
        .route("/rooms/{id}/goals/{gid}", axum::routing::patch(update_goal))
        .route(
            "/rooms/{id}/attentions",
            get(list_attentions).post(create_attention),
        )
        .route(
            "/rooms/{id}/attentions/{attention_id}/claim",
            axum::routing::post(claim_attention),
        )
        .route(
            "/rooms/{id}/attentions/{attention_id}/resolve",
            axum::routing::post(resolve_attention),
        )
        .route("/rooms/{id}/waits", get(list_waits).post(set_wait))
        .route(
            "/rooms/{id}/waits/{name}",
            axum::routing::delete(clear_wait),
        )
        .route(
            "/rooms/{id}/care/{signal_id}/ack",
            axum::routing::post(ack_care),
        )
        .route("/ws", get(ws_handler))
        .route("/birebirs", get(list_my_dms).post(open_dm_route))
        .route("/birebirs/candidates", get(dm_candidates_route))
        .route(
            "/birebirs/{room}/master",
            axum::routing::post(dm_master_route),
        )
        .route("/building/birebirs", get(dm_overview_route))
        .route("/building/birebirs/{room}/messages", get(dm_witness_route))
        .route(
            "/building/birebirs/{room}/close",
            axum::routing::post(dm_close_route),
        )
        .route(
            "/building/birebirs/{room}/seal",
            axum::routing::post(dm_seal_route),
        )
        // Innermost: by the time it runs, the direct message fence has already
        // answered for a direct message and the membership gate for a stranger.
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            extension_fence,
        ))
        .route_layer(axum::middleware::from_fn_with_state(hub.clone(), dm_fence))
        // A group loca is invisible to everyone outside its group.
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            group_fence,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            require_membership,
        ))
        .with_state(hub)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
}
