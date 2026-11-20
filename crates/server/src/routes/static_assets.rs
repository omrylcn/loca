//! The web client and the documents the server serves: `/`, `/assets/*`, the
//! principles and the getting-started guide.

use crate::*;

pub(crate) const WEB_INDEX: &str = include_str!("../../../../web/index.html");
pub(crate) const WEB_FAVICON: &str = include_str!("../../../../web/assets/favicon.svg");
pub(crate) const WEB_STYLES: &str = include_str!("../../../../web/assets/styles.css");
pub(crate) const WEB_BASE_SIDEBAR_STYLES: &str = include_str!("../../../../web/assets/sidebar.css");
pub(crate) const WEB_BASE_ROOM_STYLES: &str = include_str!("../../../../web/assets/room.css");
pub(crate) const WEB_BASE_FEED_STYLES: &str = include_str!("../../../../web/assets/feed.css");
pub(crate) const WEB_BASE_PANELS_STYLES: &str = include_str!("../../../../web/assets/panels.css");
pub(crate) const WEB_BASE_OVERLAYS_STYLES: &str =
    include_str!("../../../../web/assets/overlays.css");
pub(crate) const WEB_BASE_GUIDE_STYLES: &str = include_str!("../../../../web/assets/guide.css");
pub(crate) const WEB_BASE_PERSONAL_STYLES: &str =
    include_str!("../../../../web/assets/personal.css");
pub(crate) const WEB_PROFILE_STYLES: &str = include_str!("../../../../web/assets/profile.css");
pub(crate) const WEB_APPEARANCE_STYLES: &str =
    include_str!("../../../../web/assets/appearance.css");
pub(crate) const WEB_THEMES_STYLES: &str = include_str!("../../../../web/assets/themes.css");
pub(crate) const WEB_SOUNDS_STYLES: &str = include_str!("../../../../web/assets/sounds.css");
pub(crate) const WEB_RANKS_STYLES: &str = include_str!("../../../../web/assets/ranks.css");
pub(crate) const WEB_CARETAKER_STYLES: &str = include_str!("../../../../web/assets/caretaker.css");
pub(crate) const WEB_FLOORS_STYLES: &str = include_str!("../../../../web/assets/floors.css");
pub(crate) const WEB_BOARD_LIST_STYLES: &str = include_str!("../../../../web/assets/boardlist.css");
pub(crate) const WEB_ROSTER_STYLES: &str = include_str!("../../../../web/assets/roster.css");
pub(crate) const WEB_AVATAR: &str = include_str!("../../../../web/assets/avatar.js");
pub(crate) const WEB_STATE: &str = include_str!("../../../../web/assets/state.js");
pub(crate) const WEB_SOCKET: &str = include_str!("../../../../web/assets/socket.js");
pub(crate) const WEB_PEOPLE: &str = include_str!("../../../../web/assets/people.js");
pub(crate) const WEB_CHAT: &str = include_str!("../../../../web/assets/chat.js");
pub(crate) const WEB_ADMIN: &str = include_str!("../../../../web/assets/admin.js");
pub(crate) const WEB_FOCUS: &str = include_str!("../../../../web/assets/focus.js");
pub(crate) const WEB_NOTES: &str = include_str!("../../../../web/assets/notes.js");
pub(crate) const WEB_MEMORY: &str = include_str!("../../../../web/assets/memory.js");
pub(crate) const WEB_WIKI: &str = include_str!("../../../../web/assets/wiki.js");
pub(crate) const WEB_VAULT: &str = include_str!("../../../../web/assets/vault.js");
pub(crate) const WEB_BOARD: &str = include_str!("../../../../web/assets/board.js");
pub(crate) const WEB_BOARD_LIST: &str = include_str!("../../../../web/assets/boardlist.js");
pub(crate) const WEB_TASK_VIEW: &str = include_str!("../../../../web/assets/taskview.js");
pub(crate) const WEB_CONTINUITY: &str = include_str!("../../../../web/assets/continuity.js");
pub(crate) const WEB_LOCA_DELETE: &str = include_str!("../../../../web/assets/locadelete.js");
pub(crate) const WEB_PROFILE: &str = include_str!("../../../../web/assets/profile.js");
pub(crate) const WEB_THEME: &str = include_str!("../../../../web/assets/theme.js");
pub(crate) const WEB_SIDEBAR: &str = include_str!("../../../../web/assets/sidebar.js");
pub(crate) const WEB_API: &str = include_str!("../../../../web/assets/api.js");
pub(crate) const WEB_APP: &str = include_str!("../../../../web/assets/app.js");
pub(crate) const WEB_JOINREQUEST: &str = include_str!("../../../../web/assets/joinrequest.js");
pub(crate) const WEB_GETSTARTED: &str = include_str!("../../../../web/assets/getstarted.js");
pub(crate) const WEB_TOOLTIP: &str = include_str!("../../../../web/assets/tooltip.js");
pub(crate) const WEB_CLIPBOARD: &str = include_str!("../../../../web/assets/clipboard.js");
pub(crate) const WEB_DM: &str = include_str!("../../../../web/assets/dm.js");
pub(crate) const WEB_KEYS: &str = include_str!("../../../../web/assets/keys.js");
pub(crate) const WEB_BUILDING_SETTINGS: &str =
    include_str!("../../../../web/assets/buildingsettings.js");
pub(crate) const WEB_EXTENSIONS: &str = include_str!("../../../../web/assets/extensions.js");
pub(crate) const WEB_IDENTITY: &str = include_str!("../../../../web/assets/identity.js");
pub(crate) const WEB_GROUPS: &str = include_str!("../../../../web/assets/groups.js");
pub(crate) const WEB_KIT_VERSION: &str = include_str!("../../../../web/assets/kitversion.js");
pub(crate) const WEB_ROSTER: &str = include_str!("../../../../web/assets/roster.js");
pub(crate) const WEB_ADDRESSING: &str = include_str!("../../../../web/assets/addressing.js");
pub(crate) const WEB_LOCAINFO: &str = include_str!("../../../../web/assets/locainfo.js");
pub(crate) const WEB_THREADS: &str = include_str!("../../../../web/assets/threads.js");
pub(crate) const WEB_THREADS_STYLES: &str = include_str!("../../../../web/assets/threads.css");
pub(crate) const WEB_HOME: &str = include_str!("../../../../web/assets/home.js");
pub(crate) const WEB_ADDRESSING_STYLES: &str =
    include_str!("../../../../web/assets/addressing.css");
pub(crate) const WEB_CHATSURFACE_STYLES: &str =
    include_str!("../../../../web/assets/chatsurface.css");
pub(crate) const WEB_PAGE_CHROME_STYLES: &str =
    include_str!("../../../../web/assets/page-chrome.css");
pub(crate) const WEB_PAGE_ENTRY_STYLES: &str =
    include_str!("../../../../web/assets/page-entry.css");
pub(crate) const WEB_PAGE_PEOPLE_STYLES: &str =
    include_str!("../../../../web/assets/page-people.css");
pub(crate) const WEB_BUILDING_SETTINGS_STYLES: &str =
    include_str!("../../../../web/assets/buildingsettings.css");
pub(crate) const WEB_PAGE_MANAGE_STYLES: &str =
    include_str!("../../../../web/assets/page-manage.css");
pub(crate) const WEB_PAGE_CONTENT_STYLES: &str =
    include_str!("../../../../web/assets/page-content.css");
pub(crate) const WEB_PAGE_HOME_STYLES: &str = include_str!("../../../../web/assets/page-home.css");
pub(crate) const WEB_NOTIFICATIONS: &str = include_str!("../../../../web/assets/notifications.js");
pub(crate) const WEB_SOUNDS: &str = include_str!("../../../../web/assets/sounds.js");
pub(crate) const WEB_SEARCH: &str = include_str!("../../../../web/assets/search.js");
pub(crate) const WEB_SEARCH_STYLES: &str = include_str!("../../../../web/assets/search.css");
pub(crate) const PRINCIPLES_TR: &str = include_str!("../../../../PRINCIPLES.tr.md");
pub(crate) const PRINCIPLES_EN: &str = include_str!("../../../../PRINCIPLES.md");
pub(crate) const GETTING_STARTED_DOC: &str = include_str!("../../../../docs/getting-started.md");
/// Serve the web client with no-store so browsers never run a stale UI after a
/// server update (this bit users repeatedly during development).
pub(crate) async fn index() -> impl IntoResponse {
    (
        [
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
            (
                "permissions-policy",
                "camera=(), microphone=(), geolocation=()",
            ),
            (
                "content-security-policy",
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; \
                 style-src 'self' 'unsafe-inline'; script-src 'self'; \
                 frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ],
        Html(WEB_INDEX),
    )
}

pub(crate) fn principles_document(body: &'static str) -> impl IntoResponse {
    (
        [
            ("content-type", "text/markdown; charset=utf-8"),
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
        ],
        body,
    )
}

pub(crate) async fn principles_tr() -> impl IntoResponse {
    principles_document(PRINCIPLES_TR)
}

pub(crate) async fn principles_en() -> impl IntoResponse {
    principles_document(PRINCIPLES_EN)
}

/// The Getting Started guide links here for the full, actionable setup
/// walkthrough (self-host, Claude Code / Codex skill install). Served locally
/// so the guide never depends on a third-party host; contains no credential.
pub(crate) async fn getting_started_doc() -> impl IntoResponse {
    principles_document(GETTING_STARTED_DOC)
}

/// Serve only compile-time embedded browser assets. The allow-list prevents
/// this route from becoming a filesystem server, while `include_str!` keeps a
/// deployed binary self-contained and makes missing assets a compile failure.
pub(crate) async fn web_asset(Path(name): Path<String>) -> impl IntoResponse {
    let asset = match name.as_str() {
        "favicon.svg" => Some(("image/svg+xml; charset=utf-8", WEB_FAVICON)),
        "styles.css" => Some(("text/css; charset=utf-8", WEB_STYLES)),
        "sidebar.css" => Some(("text/css; charset=utf-8", WEB_BASE_SIDEBAR_STYLES)),
        "room.css" => Some(("text/css; charset=utf-8", WEB_BASE_ROOM_STYLES)),
        "feed.css" => Some(("text/css; charset=utf-8", WEB_BASE_FEED_STYLES)),
        "panels.css" => Some(("text/css; charset=utf-8", WEB_BASE_PANELS_STYLES)),
        "overlays.css" => Some(("text/css; charset=utf-8", WEB_BASE_OVERLAYS_STYLES)),
        "guide.css" => Some(("text/css; charset=utf-8", WEB_BASE_GUIDE_STYLES)),
        "personal.css" => Some(("text/css; charset=utf-8", WEB_BASE_PERSONAL_STYLES)),
        "profile.css" => Some(("text/css; charset=utf-8", WEB_PROFILE_STYLES)),
        "appearance.css" => Some(("text/css; charset=utf-8", WEB_APPEARANCE_STYLES)),
        "themes.css" => Some(("text/css; charset=utf-8", WEB_THEMES_STYLES)),
        "sounds.css" => Some(("text/css; charset=utf-8", WEB_SOUNDS_STYLES)),
        "ranks.css" => Some(("text/css; charset=utf-8", WEB_RANKS_STYLES)),
        "caretaker.css" => Some(("text/css; charset=utf-8", WEB_CARETAKER_STYLES)),
        "floors.css" => Some(("text/css; charset=utf-8", WEB_FLOORS_STYLES)),
        "boardlist.css" => Some(("text/css; charset=utf-8", WEB_BOARD_LIST_STYLES)),
        "roster.css" => Some(("text/css; charset=utf-8", WEB_ROSTER_STYLES)),
        "avatar.js" => Some(("text/javascript; charset=utf-8", WEB_AVATAR)),
        "state.js" => Some(("text/javascript; charset=utf-8", WEB_STATE)),
        "socket.js" => Some(("text/javascript; charset=utf-8", WEB_SOCKET)),
        "people.js" => Some(("text/javascript; charset=utf-8", WEB_PEOPLE)),
        "chat.js" => Some(("text/javascript; charset=utf-8", WEB_CHAT)),
        "admin.js" => Some(("text/javascript; charset=utf-8", WEB_ADMIN)),
        "focus.js" => Some(("text/javascript; charset=utf-8", WEB_FOCUS)),
        "board.js" => Some(("text/javascript; charset=utf-8", WEB_BOARD)),
        "boardlist.js" => Some(("text/javascript; charset=utf-8", WEB_BOARD_LIST)),
        "taskview.js" => Some(("text/javascript; charset=utf-8", WEB_TASK_VIEW)),
        "continuity.js" => Some(("text/javascript; charset=utf-8", WEB_CONTINUITY)),
        "locadelete.js" => Some(("text/javascript; charset=utf-8", WEB_LOCA_DELETE)),
        "notes.js" => Some(("text/javascript; charset=utf-8", WEB_NOTES)),
        "memory.js" => Some(("text/javascript; charset=utf-8", WEB_MEMORY)),
        "wiki.js" => Some(("text/javascript; charset=utf-8", WEB_WIKI)),
        "vault.js" => Some(("text/javascript; charset=utf-8", WEB_VAULT)),
        "profile.js" => Some(("text/javascript; charset=utf-8", WEB_PROFILE)),
        "theme.js" => Some(("text/javascript; charset=utf-8", WEB_THEME)),
        "sidebar.js" => Some(("text/javascript; charset=utf-8", WEB_SIDEBAR)),
        "api.js" => Some(("text/javascript; charset=utf-8", WEB_API)),
        "app.js" => Some(("text/javascript; charset=utf-8", WEB_APP)),
        "joinrequest.js" => Some(("text/javascript; charset=utf-8", WEB_JOINREQUEST)),
        "getstarted.js" => Some(("text/javascript; charset=utf-8", WEB_GETSTARTED)),
        "tooltip.js" => Some(("text/javascript; charset=utf-8", WEB_TOOLTIP)),
        "clipboard.js" => Some(("text/javascript; charset=utf-8", WEB_CLIPBOARD)),
        "dm.js" => Some(("text/javascript; charset=utf-8", WEB_DM)),
        "keys.js" => Some(("text/javascript; charset=utf-8", WEB_KEYS)),
        "buildingsettings.js" => Some(("text/javascript; charset=utf-8", WEB_BUILDING_SETTINGS)),
        "extensions.js" => Some(("text/javascript; charset=utf-8", WEB_EXTENSIONS)),
        "identity.js" => Some(("text/javascript; charset=utf-8", WEB_IDENTITY)),
        "groups.js" => Some(("text/javascript; charset=utf-8", WEB_GROUPS)),
        "kitversion.js" => Some(("text/javascript; charset=utf-8", WEB_KIT_VERSION)),
        "roster.js" => Some(("text/javascript; charset=utf-8", WEB_ROSTER)),
        "addressing.js" => Some(("text/javascript; charset=utf-8", WEB_ADDRESSING)),
        "locainfo.js" => Some(("text/javascript; charset=utf-8", WEB_LOCAINFO)),
        "threads.js" => Some(("text/javascript; charset=utf-8", WEB_THREADS)),
        "threads.css" => Some(("text/css; charset=utf-8", WEB_THREADS_STYLES)),
        "home.js" => Some(("text/javascript; charset=utf-8", WEB_HOME)),
        "addressing.css" => Some(("text/css; charset=utf-8", WEB_ADDRESSING_STYLES)),
        "chatsurface.css" => Some(("text/css; charset=utf-8", WEB_CHATSURFACE_STYLES)),
        "page-chrome.css" => Some(("text/css; charset=utf-8", WEB_PAGE_CHROME_STYLES)),
        "page-entry.css" => Some(("text/css; charset=utf-8", WEB_PAGE_ENTRY_STYLES)),
        "page-people.css" => Some(("text/css; charset=utf-8", WEB_PAGE_PEOPLE_STYLES)),
        "buildingsettings.css" => Some(("text/css; charset=utf-8", WEB_BUILDING_SETTINGS_STYLES)),
        "page-manage.css" => Some(("text/css; charset=utf-8", WEB_PAGE_MANAGE_STYLES)),
        "page-content.css" => Some(("text/css; charset=utf-8", WEB_PAGE_CONTENT_STYLES)),
        "page-home.css" => Some(("text/css; charset=utf-8", WEB_PAGE_HOME_STYLES)),
        "notifications.js" => Some(("text/javascript; charset=utf-8", WEB_NOTIFICATIONS)),
        "sounds.js" => Some(("text/javascript; charset=utf-8", WEB_SOUNDS)),
        "search.js" => Some(("text/javascript; charset=utf-8", WEB_SEARCH)),
        "search.css" => Some(("text/css; charset=utf-8", WEB_SEARCH_STYLES)),
        _ => None,
    };
    match asset {
        Some((content_type, body)) => (
            StatusCode::OK,
            [
                ("content-type", content_type),
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            body,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
    }
}
