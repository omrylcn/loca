//! Authority matrix: every HTTP route the router serves, called by every kind
//! of caller, against every kind of loca, held to what PRINCIPLES.md and
//! DESIGN.md say about who may call it.
//!
//! How it fits together:
//!
//! - The routes come from the router itself. `app_routes()` reads the
//!   `Router::new()` chain in `src/main.rs` (axum 0.7 cannot list its routes)
//!   and fails closed on anything it does not understand (`.nest`, `.merge`,
//!   a non-literal path, an unknown method). The live server then has to
//!   serve every listed route, so the two cannot drift silently.
//! - `ROWS` holds one row per route. `every_router_route_has_exactly_one_row`
//!   fails, without starting a server, as soon as a route exists with no row.
//!   A new route therefore cannot land without someone stating who may call
//!   it.
//! - A row names a `Rule` (who may call it, with the paragraph it comes from)
//!   and a few `Cell` exceptions. A cell is either the current behaviour where
//!   the documents say nothing (`Why::Current`, marked
//!   `// current behaviour, not stated in PRINCIPLES`), or a `KNOWN_GAP_*`
//!   constant where the current behaviour contradicts a principle. A gap cell
//!   asserts what happens today, so a fix makes the test fail until the gap
//!   is removed.
//! - `authority_matrix` builds a closed Building the real way (Master pairing,
//!   Smaster codes, admit + invite, an invite-only holder, no credential; an
//!   ordinary loca, a group loca, a direct message and Care) and calls every route.
//!
//! Outcome classes. The principles say who may act, not which status code a
//! refusal carries, so a refusal is any of 401/403/404 (`Refuse`). Where a
//! principle says the caller must not even learn that the thing exists, the
//! answer must be byte-for-byte the answer for an id that names nothing
//! (`Hidden`). `Allow` is a success (2xx, or 101 for a WebSocket); a row
//! whose target state cannot give every allowed caller a success (a second
//! create, a seal before a close) sets `merits`, which also accepts a
//! non-authority 400/409/422.
//!
//! When a row fails, the message prints the whole observed line for that
//! route; `AUTHORITY_MATRIX_DUMP=<substring>` prints the observed matrix for
//! matching routes without asserting, which is the quickest way to write the
//! row for a new route.
use super::*;

const ROOT: &str = "matrix-root-credential";
const MAIN_RS: &str = include_str!("../../src/main.rs");

// ---------------------------------------------------------------------------
// Callers and loca kinds
// ---------------------------------------------------------------------------

/// Every kind of caller, lowest first. Within one loca the matrix calls them
/// in this order, so a caller that is expected to be refused never runs after
/// one that may have changed the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Who {
    /// No credential at all.
    Outsider,
    /// A session taken with nothing but an invite for the ordinary loca.
    Invite,
    /// `loca-care`: a member agent seated in Care by its own invite.
    Caretaker,
    /// A human Building member with no seat anywhere.
    Lobby,
    /// An agent member seated in the ordinary loca, a member of the group
    /// and the second person of the direct message.
    Participant,
    /// A human member seated in the ordinary loca and appointed its Operator;
    /// the opener of the direct message.
    Operator,
    /// A second Smaster, outside the group.
    OtherSmaster,
    /// A Smaster; founder (and so Operator) of the group and its loca.
    Smaster,
    /// The Master, signed in through a one-use pairing code.
    Master,
}

const CALLERS: [Who; 9] = [
    Who::Outsider,
    Who::Invite,
    Who::Caretaker,
    Who::Lobby,
    Who::Participant,
    Who::Operator,
    Who::OtherSmaster,
    Who::Smaster,
    Who::Master,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Kind {
    /// `atelier`: an ordinary loca created by the Master.
    Ordinary,
    /// `squad-atelier`: the loca of the Smaster's group (members: the
    /// Participant and a spare agent).
    Group,
    /// The Operator's direct message with the Participant.
    Dm,
    /// `iye`: the reserved home loca.
    CareLoca,
}

const KINDS: [Kind; 4] = [Kind::Ordinary, Kind::Group, Kind::Dm, Kind::CareLoca];

// ---------------------------------------------------------------------------
// Expectations
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    /// Success: 2xx (101 for a WebSocket). With `Row::merits`, also a
    /// non-authority 400/409/422.
    Allow,
    /// Refused at the door: 401, 403 or 404.
    Refuse,
    /// Refused, and the answer is identical (status and body) to the answer
    /// for an id that names nothing: the caller learns nothing.
    Hidden,
    /// The caller passed the door and the handler answered that the target
    /// is not there (404 that differs from the nonexistent-id answer).
    Absent,
    /// The caller passed the door and the request was turned down on its
    /// merits (400/409/422), not by authority.
    Rejected,
}

/// Why a cell departs from its row's rule.
#[derive(Clone, Copy, Debug)]
enum Why {
    /// The documents do not say; this is what the server does today.
    Current(&'static str),
    /// The documents say otherwise; this is what the server does today.
    /// No gap is open at the moment (the last three were closed); the variant
    /// and `gap()` stay so the next one found has somewhere to be written.
    #[allow(dead_code)]
    Gap(&'static Gap),
}

/// A place where the server contradicts PRINCIPLES.md or DESIGN.md. The row
/// asserts the current behaviour; fixing the server makes the row fail until
/// the cell is removed.
#[derive(Debug)]
struct Gap {
    id: &'static str,
    happens: &'static str,
    principle: &'static str,
    severity: &'static str,
}

/// One departure from a row's rule. `kind: None` covers every loca kind (and
/// the single cell of a route that names no loca).
#[derive(Clone, Copy, Debug)]
struct Cell {
    who: Who,
    kind: Option<Kind>,
    want: Want,
    why: Why,
}

const fn cur(who: Who, kind: Option<Kind>, want: Want, note: &'static str) -> Cell {
    Cell {
        who,
        kind,
        want,
        why: Why::Current(note),
    }
}

#[allow(dead_code)] // no known gap is open; see `Why::Gap`
const fn gap(who: Who, kind: Option<Kind>, want: Want, gap: &'static Gap) -> Cell {
    Cell {
        who,
        kind,
        want,
        why: Why::Gap(gap),
    }
}

/// How much of the world a row may change, and so how much it gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Iso {
    /// Reads and additive writes: run in the one shared world.
    Shared,
    /// Changes something other rows rely on: a world of its own.
    Row,
    /// Consumes its target: a world per caller.
    Caller,
}

/// Whether a row's rule comes from the documents or records today's server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Basis {
    Doc,
    /// current behaviour, not stated in PRINCIPLES
    Current,
}

struct Row {
    route: &'static str,
    rule: Rule,
    basis: Basis,
    iso: Iso,
    merits: bool,
    /// The target does not exist in this world: a caller who may act
    /// reaches the handler and is told so (`Want::Absent`).
    absent: bool,
    cells: &'static [Cell],
    /// A second body for a route already in `ROWS` (see `BODY_VARIANTS`):
    /// its name, the key `body_for` answers. `None` for the route's own row.
    body: Option<&'static str>,
}

const fn row(route: &'static str, rule: Rule, cells: &'static [Cell]) -> Row {
    Row {
        route,
        rule,
        basis: Basis::Doc,
        iso: Iso::Shared,
        merits: false,
        absent: false,
        cells,
        body: None,
    }
}

impl Row {
    /// current behaviour, not stated in PRINCIPLES
    const fn current(mut self) -> Row {
        self.basis = Basis::Current;
        self
    }
    const fn iso(mut self, iso: Iso) -> Row {
        self.iso = iso;
        self
    }
    const fn merits(mut self) -> Row {
        self.merits = true;
        self
    }
    const fn absent(mut self) -> Row {
        self.absent = true;
        self
    }
    const fn body(mut self, name: &'static str) -> Row {
        self.body = Some(name);
        self
    }
    /// The row's name in reports: the route, or its body variant.
    const fn name(&self) -> &'static str {
        match self.body {
            Some(name) => name,
            None => self.route,
        }
    }
}

// ---------------------------------------------------------------------------
// Rules: who may call, derived once from the documents
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Rule {
    /// Anyone, with or without a credential.
    Public,
    /// Nobody in this matrix (the route wants a credential none of them
    /// presents, e.g. a join-request secret).
    Nobody,
    /// Anyone holding a session.
    Holder,
    /// A principal-bound Building identity: not an invite-only session.
    Profile,
    /// Building members holding a membership key (the Lobby door).
    Membership,
    /// The Master and either Smaster.
    Ranks,
    MasterOnly,
    CaretakerOnly,
    /// Exactly these callers (Building routes).
    Only(&'static [Who]),
    /// Whoever may enter the loca. A direct message's two people included.
    Room,
    /// Whoever may enter the loca, for a feature a direct message does not have.
    Feature,
    /// The loca's Operator, the Master and Smasters (management; see
    /// `manages`).
    Manage,
    /// Building rank acting on a loca (the Master, either Smaster).
    RankManage,
    /// Master alone acting on a loca.
    MasterManage,
    /// Calling or inviting an existing member into a loca.
    Seat,
    /// Moving a loca between groups.
    MoveGroup,
    /// A direct message's two people (the route means nothing elsewhere).
    DmPair,
    /// The Master acting on a direct message it can see (close, seal).
    DmMaster,
    /// Reading a direct message's words from outside it (the Master's witness view).
    DmWitness,
    /// A group's founder and the Master.
    GroupManage,
    /// The group record: its founder and the Master.
    GroupView,
    /// Opening a direct message with the Participant.
    OpenDm,
    /// The owner alone; anyone else gets the answer for an id that names
    /// nothing.
    Own(Who),
    /// The loca's agent alone (see `the_agent`).
    Agent,
    /// The loca's agent, or whoever manages the loca.
    AgentOrManage,
    /// Any agent who may enter the loca.
    SeatedAgent,
}

/// What a caller is at a loca's door.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Door {
    In,
    /// Refused; the loca's existence is not a secret from this caller.
    Out,
    /// Refused, and must learn nothing.
    Hidden,
}

/// The door of each kind of loca.
///
/// - Ordinary. PRINCIPLES "Membership layer": an invited participant "has a
///   seat in one loca"; "The membership key opens no loca door". "Master …
///   retains Building management authority across locas"; "Smaster … manage
///   locas". `loca-care` "gains no seat in the source Loca" and "sits only in
///   Care". "Outside … must become a member before they can enter."
/// - Group. PRINCIPLES "Groups": "Only the group's members see and enter it";
///   "The Master sees everything"; "outsiders do not learn that a group loca
///   exists"; "The rule is the same at every door (sidebar, REST, WS,
///   search)". DESIGN "Gruplar" (groups): "Everyone outside, the Smaster included, gets
///   the answer a Loca that does not exist gets … A Smaster outside the group does not
///   see the group."
/// - Direct message. PRINCIPLES "Birebir": "only two people sit in and only those
///   two see"; "No rank watches from the door"; "The Master sees only that it
///   exists … there is no look path for the Master. A Smaster does not even
///   see that they exist." Callers below the Smaster are held to the same
///   (PRINCIPLES "Hierarchy": "A higher layer contains the ones below it").
/// - Care. PRINCIPLES "İye": "only Master, Smaster, and loca-care may be there
///   … No other building member can enter Care through a call or invitation."
fn door(who: Who, kind: Kind) -> Door {
    use Who::*;
    match kind {
        Kind::Ordinary => match who {
            Master | Smaster | OtherSmaster | Operator | Participant | Invite => Door::In,
            _ => Door::Out,
        },
        Kind::Group => match who {
            Master | Smaster | Participant => Door::In,
            _ => Door::Hidden,
        },
        Kind::Dm => match who {
            Operator | Participant => Door::In,
            Master => Door::Out,
            _ => Door::Hidden,
        },
        Kind::CareLoca => match who {
            Master | Smaster | OtherSmaster | Caretaker => Door::In,
            _ => Door::Out,
        },
    }
}

fn at_door(who: Who, kind: Kind, allowed: bool) -> Want {
    match door(who, kind) {
        Door::In if allowed => Want::Allow,
        Door::Hidden => Want::Hidden,
        _ => Want::Refuse,
    }
}

/// Who manages a loca. PRINCIPLES "Loca layer": the Operator is "responsible
/// for the room's operation: mode, turn order, muting, and moderation";
/// "Smaster retains Loca-management authority through Building rank";
/// "Master … retains Building management authority across locas". The group
/// loca's founder Smaster holds its Operator seat ("Human Master/Smaster
/// founders receive the appointment atomically"). PRINCIPLES "Birebir": "There
/// are no tasks, goals, wiki, notes, operator, or lead". Care has no appointed
/// Operator; Master and Smaster "enter by rank".
fn manages(who: Who, kind: Kind) -> bool {
    use Who::*;
    match kind {
        Kind::Ordinary => matches!(who, Master | Smaster | OtherSmaster | Operator),
        Kind::Group => matches!(who, Master | Smaster),
        Kind::CareLoca => matches!(who, Master | Smaster | OtherSmaster),
        Kind::Dm => false,
    }
}

/// The agent of each loca in this world. It is the loca's Lead, its wiki
/// editor, the owner named by its archived memory record and the declarer of
/// its seeded wait, so a rule about any of those roles names it. A direct message
/// has none of them.
fn the_agent(kind: Kind) -> Option<Who> {
    match kind {
        Kind::Ordinary | Kind::Group => Some(Who::Participant),
        Kind::CareLoca => Some(Who::Caretaker),
        Kind::Dm => None,
    }
}

/// A Building rank: the Master or either Smaster.
fn rank(who: Who) -> bool {
    matches!(who, Who::Master | Who::Smaster | Who::OtherSmaster)
}

fn want_for(rule: Rule, route: &str, who: Who, kind: Option<Kind>) -> (Want, Option<&'static Gap>) {
    use Who::*;
    let yes = |b: bool| if b { Want::Allow } else { Want::Refuse };
    let want = match (rule, kind) {
        (Rule::Public, _) => Want::Allow,
        (Rule::Nobody, _) => Want::Refuse,
        (Rule::Holder, _) => yes(who != Outsider),
        (Rule::Profile, _) => yes(!matches!(who, Outsider | Invite)),
        (Rule::Membership, _) => yes(matches!(who, Operator | Participant | Lobby | Caretaker)),
        (Rule::Ranks, _) => yes(rank(who)),
        (Rule::MasterOnly, _) => yes(who == Master),
        (Rule::CaretakerOnly, _) => yes(who == Caretaker),
        (Rule::Only(set), _) => yes(set.contains(&who)),
        (Rule::GroupView, _) => {
            if matches!(who, Master | Smaster) {
                Want::Allow
            } else {
                Want::Hidden
            }
        }
        (Rule::GroupManage, _) => match who {
            Master | Smaster => Want::Allow,
            Participant => Want::Refuse,
            _ => Want::Hidden,
        },
        (Rule::OpenDm, _) => yes(who == Operator || rank(who)),
        (Rule::Own(owner), _) => {
            if who == owner {
                Want::Allow
            } else {
                Want::Hidden
            }
        }
        (_, None) => panic!("{route}: a loca rule on a route that names no loca"),
        (Rule::Room, Some(k)) => at_door(who, k, true),
        (Rule::Feature, Some(k)) => at_door(who, k, k != Kind::Dm),
        (Rule::Manage, Some(k)) => at_door(who, k, manages(who, k)),
        (Rule::RankManage, Some(k)) => at_door(who, k, k != Kind::Dm && rank(who)),
        (Rule::MasterManage, Some(k)) => at_door(who, k, k != Kind::Dm && who == Master),
        (Rule::Seat, Some(k)) => at_door(
            who,
            k,
            matches!(k, Kind::Ordinary | Kind::Group) && rank(who),
        ),
        (Rule::MoveGroup, Some(k)) => at_door(
            who,
            k,
            matches!(k, Kind::Ordinary | Kind::Group) && who == Master,
        ),
        (Rule::DmPair, Some(k)) => at_door(who, k, k == Kind::Dm),
        (Rule::DmMaster, Some(k)) => {
            if k == Kind::Dm && who == Master {
                Want::Allow
            } else {
                at_door(who, k, false)
            }
        }
        (Rule::DmWitness, Some(k)) => at_door(who, k, false),
        (Rule::Agent, Some(k)) => at_door(who, k, the_agent(k) == Some(who)),
        (Rule::AgentOrManage, Some(k)) => {
            at_door(who, k, the_agent(k) == Some(who) || manages(who, k))
        }
        (Rule::SeatedAgent, Some(k)) => at_door(
            who,
            k,
            k != Kind::Dm && matches!(who, Participant | Invite | Caretaker),
        ),
    };
    (want, None)
}

// ---------------------------------------------------------------------------
// The table: one row per route the router serves
// ---------------------------------------------------------------------------

use Iso::{Caller as PerCaller, Row as PerRow};
use Kind::{CareLoca as I, Group as G, Ordinary as O};
use Want::{Absent, Allow, Hidden, Refuse, Rejected};
use Who::{
    Invite as Dv, Master as Ms, Operator as Op, OtherSmaster as Sm2, Participant as Pt,
    Smaster as Sm,
};

/// Routes whose answer depends on what the body asks for, called again with
/// a second body under the same rule. Each runs in a world of its own.
const BODY_VARIANTS: &[Row] = &[
    // PRINCIPLES "A loca's identity" and "Loca extensions": the loca's
    // identity is changed by whoever changes its extensions (the Operator,
    // the Master, Smasters); a direct message has none. Outsiders of a floor loca
    // (the other Smaster included) get the answer for a loca that is not
    // there.
    row("PUT /rooms/:id/settings", Rule::Manage, &[])
        .body("PUT /rooms/:id/settings {identity}")
        .iso(PerRow),
];

const ROWS: &[Row] = &[
    // --- Public surface -------------------------------------------------
    // current behaviour, not stated in PRINCIPLES: the web shell, its
    // assets, the principles, the onboarding guide and the health probe are
    // open to anyone (routes/access.rs `is_public_path`).
    row("GET /", Rule::Public, &[]).current(),
    row("GET /PRINCIPLES.md", Rule::Public, &[]).current(),
    row("GET /PRINCIPLES.tr.md", Rule::Public, &[]).current(),
    row("GET /PRINCIPLES.en.md", Rule::Public, &[]).current(),
    row("GET /docs/getting-started.md", Rule::Public, &[]).current(),
    row("GET /assets/:name", Rule::Public, &[]).current(),
    row("GET /health", Rule::Public, &[]).current(),
    // PRINCIPLES "Membership layer": "Outside — may have the skill, but has
    // no building membership." The skill is downloadable before any identity.
    row("GET /downloads/skills", Rule::Public, &[]),
    row("GET /downloads/skills/:name", Rule::Public, &[]),
    row("GET /downloads/skills/:name/manifest", Rule::Public, &[]),
    // --- Sessions and Master credentials --------------------------------
    // current behaviour, not stated in PRINCIPLES: a session is not a
    // credential to mint another session; every caller here presents only a
    // session, so every caller is refused.
    row("POST /sessions", Rule::Nobody, &[]).current(),
    // PRINCIPLES "Authority and departure matrix": a Participant has "Own
    // membership/session only" — ending one's own session needs only it.
    row("DELETE /sessions", Rule::Holder, &[]).iso(PerRow),
    // PRINCIPLES "Identity, login, and authority": "one Master, many
    // credentials"; DESIGN "Credential": the root key stays in the server.
    row("POST /pairings", Rule::MasterOnly, &[]),
    // --- Admission ------------------------------------------------------
    // PRINCIPLES "Building layer": a Smaster "can admit members"; "Three
    // authority/credential layers": loca-care "gains no Building authority …
    // and makes no admit/invite decision itself".
    row("GET /admission-stock", Rule::Ranks, &[]),
    row("POST /admission-stock", Rule::Ranks, &[]),
    row("GET /join-requests", Rule::Ranks, &[]),
    row("POST /join-requests/:id/approve", Rule::Ranks, &[]).merits(),
    row("POST /join-requests/:id/deny", Rule::Ranks, &[]).merits(),
    // current behaviour, not stated in PRINCIPLES: an outsider asks to be
    // admitted without a credential; a request is then read, bootstrapped
    // and acknowledged only with its own secret, which no caller here holds.
    row("POST /join-requests", Rule::Public, &[])
        .current()
        .merits(),
    row("GET /join-requests/:id", Rule::Nobody, &[]).current(),
    row("POST /join-requests/:id/bootstrap", Rule::Nobody, &[]).current(),
    row("POST /join-requests/:id/ack", Rule::Nobody, &[]).current(),
    // current behaviour, not stated in PRINCIPLES: claiming a membership key
    // needs a live invite presented as the credential itself (`x-room-token`;
    // a membership key is refused), and acknowledging one needs that invite
    // (live or not) and the set-up's own claim id. Every caller here presents
    // a session, never an invite or a claim id, so every caller is refused.
    row("POST /membership/claim", Rule::Nobody, &[]).current(),
    row("POST /membership/claim/ack", Rule::Nobody, &[]).current(),
    // --- Profile: one's own identity ------------------------------------
    // PRINCIPLES "Authority and departure matrix": "Own membership/session
    // only". "Three authority/credential layers are not interchangeable": an
    // invite "seats that existing member only in an already-created,
    // specifically named loca" — it is not a Building profile.
    row("GET /whoami", Rule::Holder, &[]),
    row("GET /profile", Rule::Profile, &[]),
    // current behaviour, not stated in PRINCIPLES: the ranks rename
    // themselves here; members are refused.
    row("PUT /profile", Rule::Only(&[Sm2, Sm, Ms]), &[])
        .current()
        .iso(PerRow),
    // DESIGN "Kişisel görünüm": preferences belong to the principal.
    row("GET /profile/preferences", Rule::Profile, &[]),
    row("PUT /profile/preferences", Rule::Profile, &[]),
    // The caller's own full name and icon: display only, no authority, so
    // every verified principal sets its own and nobody names another.
    row("PUT /profile/card", Rule::Profile, &[]),
    // PRINCIPLES "Identity, login, and authority": "one principal may have
    // many credentials"; DESIGN "Profile": credential create/list/revoke.
    row("GET /profile/credentials", Rule::Profile, &[]),
    row("POST /profile/credentials", Rule::Profile, &[]),
    // The target is the Participant's own spare credential ("Own
    // membership/session only"). That everyone else, the Master included,
    // gets the answer for a credential that does not exist is current
    // behaviour, not stated in PRINCIPLES.
    row(
        "DELETE /profile/credentials/:credential_id",
        Rule::Own(Pt),
        &[],
    ),
    // --- Building administration views ----------------------------------
    // PRINCIPLES "Building layer": Smaster "can admit members, issue
    // invitations, and manage locas"; DESIGN "REST": `/profiles` is a
    // "Building-admin view".
    row("GET /building/ranks", Rule::Ranks, &[]),
    row("GET /building/bans", Rule::Ranks, &[]),
    row("GET /members", Rule::Ranks, &[]),
    row("POST /members", Rule::Ranks, &[]),
    row("GET /profiles", Rule::Ranks, &[]),
    row("GET /residents", Rule::Ranks, &[]),
    // PRINCIPLES "Building layer": "Building membership lifecycle actions are
    // Master-only" (suspend, resume, revoke).
    row("DELETE /members/:token", Rule::MasterOnly, &[]).iso(PerRow),
    row("POST /members/:token/suspension", Rule::MasterOnly, &[]).iso(PerRow),
    // PRINCIPLES "Building layer": a smaster "cannot appoint a new
    // smaster—authority originates with the master and nowhere else".
    row("POST /smasters", Rule::MasterOnly, &[]),
    row("DELETE /smasters/:token", Rule::MasterOnly, &[]).iso(PerRow),
    // current behaviour, not stated in PRINCIPLES: listing and renaming
    // Smasters, and the key ledger, are the Master's alone. (POST /keys with
    // kind=admission is refused to a Smaster although POST /admission-stock
    // lets it mint the same right.)
    row("GET /smasters", Rule::MasterOnly, &[]).current(),
    row("PUT /smasters/:token", Rule::MasterOnly, &[]).current(),
    row("GET /keys", Rule::MasterOnly, &[]).current(),
    row("POST /keys", Rule::MasterOnly, &[]).current(),
    row("DELETE /keys/:id", Rule::MasterOnly, &[])
        .current()
        .iso(PerRow),
    // --- Lobby and caretaker --------------------------------------------
    // PRINCIPLES "Membership layer": "An agent remains reachable through a
    // loca-independent Lobby presence connection for as long as building
    // membership survives. The membership key opens no loca door; it proves
    // identity only on that connection." An invite is not a membership.
    // (Ranks hold no membership key, so they have no Lobby connection.)
    row("GET /lobby/ws", Rule::Membership, &[]),
    // PRINCIPLES "Roles": loca-care "may use its own membership for the
    // read-only `GET /care/residents` audit".
    // (The ranks are refused this view as well; the documents grant it to
    // the caretaker and say nothing of anyone else.)
    row("GET /care/residents", Rule::CaretakerOnly, &[]),
    // current behaviour, not stated in PRINCIPLES
    row("GET /care/memory", Rule::CaretakerOnly, &[]).current(),
    // current behaviour, not stated in PRINCIPLES: runtime health is reported
    // with a raw membership key; a session is not accepted.
    row("POST /runtime/health", Rule::Nobody, &[]).current(),
    // --- Groups ---------------------------------------------------------
    // current behaviour, not stated in PRINCIPLES: the list is filtered to
    // what the caller may see; any session may ask.
    row("GET /groups", Rule::Holder, &[]).current(),
    // PRINCIPLES "Groups": "The Master or a Smaster creates a group".
    row("POST /groups", Rule::Ranks, &[]),
    // PRINCIPLES "Groups": "Another Smaster cannot see the group"; DESIGN
    // "Gruplar": "Grup dışındaki Smaster grubu görmez".
    row(
        "GET /groups/:id",
        Rule::GroupView,
        &[
            // current behaviour, not stated in PRINCIPLES: an ordinary member
            // reads its own group's record.
            cur(Pt, None, Allow, "a member reads its own group"),
        ],
    ),
    // PRINCIPLES "Groups": "the founder adds and removes the other people …
    // nobody adds themselves. The Master can manage every group".
    row("PATCH /groups/:id", Rule::GroupManage, &[]),
    row("DELETE /groups/:id", Rule::GroupManage, &[])
        .iso(PerRow)
        .merits(),
    row(
        "PUT /groups/:id/members/:principal_id",
        Rule::GroupManage,
        &[],
    ),
    row(
        "DELETE /groups/:id/members/:principal_id",
        Rule::GroupManage,
        &[],
    )
    .iso(PerRow),
    // current behaviour, not stated in PRINCIPLES: the change log is read by
    // whoever manages the group ("Every change is recorded").
    row("GET /groups/:id/log", Rule::GroupManage, &[]).current(),
    // DESIGN "Gruplar" (groups): "Only the Master moves a Loca between groups.
    // A direct message and Care are not tied to a group."
    // (Moving the ordinary loca ends the invite holder's seat, so the Master's
    // move runs in a world of its own.)
    row("PUT /rooms/:id/group", Rule::MoveGroup, &[]).iso(PerCaller),
    // current behaviour, not stated in PRINCIPLES: the preview of a move
    // follows the move itself.
    row("GET /rooms/:id/group/preview", Rule::MoveGroup, &[]).current(),
    // --- Notifications --------------------------------------------------
    // PRINCIPLES "Building notifications": "Each person sees only what was
    // sent to them"; "Who sends: the Master to everyone; a Smaster only to
    // the groups it belongs to. Nobody else sends a notification."
    row("GET /notifications", Rule::Profile, &[]),
    row("POST /notifications", Rule::MasterOnly, &[]),
    row(
        "POST /notifications/:id/read",
        Rule::Profile,
        &[
            // current behaviour, not stated in PRINCIPLES: "everyone" does not
            // include the sender.
            cur(Ms, None, Hidden, "the sender is not among the recipients"),
        ],
    ),
    // current behaviour, not stated in PRINCIPLES
    row("POST /notifications/read-all", Rule::Profile, &[]).current(),
    row("GET /notifications/sent", Rule::Ranks, &[]).current(),
    row("GET /notifications/audiences", Rule::Holder, &[]).current(),
    // --- Locas: listing and creation ------------------------------------
    // DESIGN "Sidebar": "Your Locas — erişilebilen Localar"; the list is
    // filtered by the same door.
    row("GET /rooms", Rule::Holder, &[]),
    // PRINCIPLES "Loca layer": "Human Master/Smaster founders receive the
    // appointment atomically when explicitly creating a loca. Joining,
    // messaging or issuing an invitation cannot create a missing loca".
    row("POST /rooms", Rule::Ranks, &[]),
    // current behaviour, not stated in PRINCIPLES
    row("GET /extensions", Rule::Holder, &[]).current(),
    // --- Conversation: whoever may enter --------------------------------
    // PRINCIPLES "Birebir": "History, attachments, reactions, and
    // notifications travel the loca's path"; "Every word in a direct message is said
    // to the other person". "Groups": "The rule is the same at every door
    // (sidebar, REST, WS, search)".
    row("GET /ws", Rule::Room, &[]),
    row("GET /rooms/:id/messages", Rule::Room, &[]),
    row("POST /rooms/:id/messages", Rule::Room, &[]),
    row("POST /rooms/:id/attachments", Rule::Room, &[]),
    row("GET /rooms/:id/attachments/:att_id", Rule::Room, &[]),
    row("GET /rooms/:id/reactions", Rule::Room, &[]),
    // Reacting to one's own message is refused on its merits.
    row(
        "POST /rooms/:id/messages/:message_id/reactions",
        Rule::Room,
        &[],
    )
    .merits(),
    row("GET /rooms/:id/members", Rule::Room, &[]),
    row("GET /rooms/:id/search", Rule::Room, &[]),
    // current behaviour, not stated in PRINCIPLES: mode, settings, the
    // extension set, reminders and waits are readable by whoever may enter,
    // in a direct message too.
    row("GET /rooms/:id/mode", Rule::Room, &[]).current(),
    row("GET /rooms/:id/settings", Rule::Room, &[]).current(),
    row("GET /rooms/:id/extensions", Rule::Room, &[]).current(),
    row("GET /rooms/:id/attentions", Rule::Room, &[]).current(),
    row("GET /rooms/:id/waits", Rule::Room, &[]).current(),
    // --- Management: Operator, Master, Smaster --------------------------
    // PRINCIPLES "Loca layer": the Operator decides "mode, turn order,
    // muting, and moderation"; a Lead "cannot moderate"; "If lead and
    // operator conflict, the operator wins". "Birebir": "no … operator, or
    // lead". "Loca extensions": "the operator, the Master, and Smasters change
    // it"; "A birebir has no extensions".
    row("PUT /rooms/:id/mode", Rule::Manage, &[]),
    row("PUT /rooms/:id/settings", Rule::Manage, &[]),
    row("POST /rooms/:id/lead", Rule::Manage, &[]),
    row("POST /rooms/:id/moderate", Rule::Manage, &[]),
    row("PUT /rooms/:id/extensions", Rule::Manage, &[]),
    row("PUT /rooms/:id/extensions/:ext", Rule::Manage, &[]),
    row("DELETE /rooms/:id/extensions/:ext", Rule::Manage, &[]).iso(PerRow),
    // current behaviour, not stated in PRINCIPLES: the mute/ban list is read
    // by whoever may enter; a direct message has none.
    row("GET /rooms/:id/moderate", Rule::Feature, &[]).current(),
    // PRINCIPLES "Birebir": "no … operator".
    row("GET /rooms/:id/operators", Rule::Feature, &[]),
    // DESIGN "Operator": "A Smaster may fill an empty explicit Operator
    // seat; it cannot overwrite an existing assignment by racing"; "An explicit Operator
    // can only be an active human principal" (the probe names an agent, so an
    // allowed caller is turned down on its merits and nothing changes).
    row("POST /rooms/:id/operators", Rule::RankManage, &[]).merits(),
    // PRINCIPLES "Building layer": Smaster "if it conflicts with the Master,
    // the Master wins"; DESIGN: "Smaster … cannot undo the Master's decision". The
    // ordinary loca's Operator was appointed by the Master; the group loca's
    // is its founder Smaster.
    row(
        "DELETE /rooms/:id/operators",
        Rule::RankManage,
        &[
            cur(Sm, Some(O), Refuse, "Master's appointment"),
            cur(Sm2, Some(O), Refuse, "Master's appointment"),
            // current behaviour, not stated in PRINCIPLES: Care has no appointed
            // Operator to remove.
            cur(Sm, Some(I), Absent, "no appointed operator in Care"),
            cur(Sm2, Some(I), Absent, "no appointed operator in Care"),
            cur(Ms, Some(I), Absent, "no appointed operator in Care"),
        ],
    )
    .iso(PerCaller),
    // PRINCIPLES "Building layer": "Membership is granted by the master,
    // invitations originate there"; Smaster "can … issue invitations". An
    // Operator's authority "ends at that loca's door" and is not invitation.
    // "Birebir": "There is no third seat". "İye": "No other building member
    // can enter Care through a call or invitation" (the probe calls a spare
    // member, not the caretaker).
    row("POST /rooms/:id/call", Rule::Seat, &[]),
    row("POST /rooms/:id/invites", Rule::Seat, &[]).merits(),
    row("GET /rooms/:id/invites", Rule::RankManage, &[]),
    // PRINCIPLES "Building layer": a smaster "cannot revoke an invitation
    // issued by the master". The target is a Master-issued invite in the
    // ordinary loca; elsewhere it names no invite of that loca.
    row(
        "DELETE /rooms/:id/invites/:token",
        Rule::RankManage,
        &[
            cur(Sm, Some(O), Refuse, "issued by the Master"),
            cur(Sm2, Some(O), Refuse, "issued by the Master"),
            // current behaviour, not stated in PRINCIPLES: another loca's invite
            // is not found here.
            cur(Sm, Some(G), Absent, "the invite belongs to another loca"),
            cur(Ms, Some(G), Absent, "the invite belongs to another loca"),
            cur(Sm, Some(I), Absent, "the invite belongs to another loca"),
            cur(Sm2, Some(I), Absent, "the invite belongs to another loca"),
            cur(Ms, Some(I), Absent, "the invite belongs to another loca"),
        ],
    )
    .iso(PerCaller),
    // PRINCIPLES "Authority and departure matrix": "Seal | Permanent
    // Master-only decision". A seal needs a close first, so the Master is
    // turned down on its merits here.
    row("DELETE /rooms/:id", Rule::MasterManage, &[])
        .iso(PerRow)
        .merits(),
    // current behaviour, not stated in PRINCIPLES: release ends the seat
    // named by an invite or a loca-scoped session. A member signed in with its
    // membership key (the Operator, the Participant, the caretaker) is
    // refused although it holds a seat; only the invite-scoped session leaves.
    row(
        "POST /rooms/:id/release",
        Rule::Only(&[]),
        &[cur(
            Dv,
            Some(O),
            Allow,
            "the invite-scoped session releases its seat",
        )],
    )
    .current()
    .iso(PerRow),
    // --- Work: tasks, goals, reminders, waits ---------------------------
    // PRINCIPLES "Roles": "A task … is born with an operator's signature; an
    // agent claims and completes it, and the operator may object by removing
    // or reopening it." "A goal … The operator creates it … Agents may
    // suggest progress, but they cannot open or change a goal". "Birebir": no
    // tasks or goals.
    row("GET /rooms/:id/tasks", Rule::Feature, &[]),
    row("POST /rooms/:id/tasks", Rule::Manage, &[]),
    // The probe claims the seeded task for the caller itself.
    row("PATCH /rooms/:id/tasks/:tid", Rule::Feature, &[]).iso(PerCaller),
    row("GET /rooms/:id/goals", Rule::Feature, &[]),
    // One active goal at a time: a second is refused on its merits.
    row("POST /rooms/:id/goals", Rule::Manage, &[]).merits(),
    row("PATCH /rooms/:id/goals/:gid", Rule::Manage, &[]),
    // PRINCIPLES "A signal has one accountable lifecycle": "The operator
    // selects the dynamic room lead, one named person, or everyone in the
    // loca as the Reminder audience".
    row("POST /rooms/:id/attentions", Rule::Manage, &[]),
    // PRINCIPLES "Loca layer": "An active lead … is the single first owner of
    // care signals; that visibility grants no additional operator
    // authority." The seeded reminder is addressed to the Lead.
    row(
        "POST /rooms/:id/attentions/:attention_id/claim",
        Rule::Agent,
        &[],
    )
    .merits(),
    // current behaviour, not stated in PRINCIPLES: the Lead or a manager
    // resolves it.
    row(
        "POST /rooms/:id/attentions/:attention_id/resolve",
        Rule::AgentOrManage,
        &[],
    )
    .current()
    .merits(),
    // current behaviour, not stated in PRINCIPLES: there is no care signal
    // to acknowledge; whoever may enter reaches the handler and is told so.
    row("POST /rooms/:id/care/:signal_id/ack", Rule::Feature, &[])
        .current()
        .absent(),
    // PRINCIPLES "Silence is not intent": "An agent declares that it is
    // waiting, who it is waiting for, and why."
    row(
        "POST /rooms/:id/waits",
        Rule::Feature,
        &[
            // current behaviour, not stated in PRINCIPLES: a human Operator's
            // wait is refused on its merits; the ranks may declare one.
            cur(Op, Some(O), Rejected, "a human does not declare a wait"),
        ],
    ),
    // current behaviour, not stated in PRINCIPLES: the seeded wait is the
    // agent's own; the declarer or a manager clears it.
    row("DELETE /rooms/:id/waits/:name", Rule::AgentOrManage, &[])
        .current()
        .iso(PerCaller),
    // --- Notes, journal, wiki, memory: features of a loca --------------
    // PRINCIPLES "Loca extensions": "Notes · Journal, Loca Wiki … become an
    // extension"; "A birebir has no extensions". Who among the seated may
    // write is not stated.
    row("GET /rooms/:id/notes", Rule::Feature, &[]),
    row("GET /rooms/:id/notes/:key", Rule::Feature, &[]),
    row("GET /rooms/:id/notes/:key/history", Rule::Feature, &[]),
    // current behaviour, not stated in PRINCIPLES: every seated caller,
    // invite holder included, creates, edits and deletes a note.
    row("POST /rooms/:id/notes", Rule::Feature, &[]).current(),
    row("PUT /rooms/:id/notes/:key", Rule::Feature, &[]).current(),
    row("DELETE /rooms/:id/notes/:key", Rule::Feature, &[])
        .current()
        .iso(PerCaller),
    row("GET /rooms/:id/journal", Rule::Feature, &[]),
    // current behaviour, not stated in PRINCIPLES
    row("POST /rooms/:id/journal", Rule::Feature, &[]).current(),
    // PRINCIPLES "Loca layer": "Wiki editor — explicitly appointed by the
    // operator".
    row("PUT /rooms/:id/wiki/config", Rule::Manage, &[]),
    row("GET /rooms/:id/wiki", Rule::Feature, &[]),
    row("GET /rooms/:id/wiki/pages/:slug", Rule::Feature, &[]),
    row("GET /rooms/:id/wiki/maintenance", Rule::Feature, &[]),
    // The editor (the loca's agent here) "maintain[s] the Loca's
    // source-linked common context": review and maintenance are its own.
    row("POST /rooms/:id/wiki/review", Rule::Agent, &[]).merits(),
    row("POST /rooms/:id/wiki/maintenance/claim", Rule::Agent, &[]).merits(),
    row("POST /rooms/:id/wiki/maintenance/finish", Rule::Agent, &[]).merits(),
    // current behaviour, not stated in PRINCIPLES: the Operator edits a page
    // directly; only the editor removes one or renews its lease; any seated
    // agent declares a native runtime and asks for context; offers and the
    // request list are for the editor and the managers.
    row("PUT /rooms/:id/wiki/pages/:slug", Rule::Manage, &[])
        .current()
        .merits(),
    row("DELETE /rooms/:id/wiki/pages/:slug", Rule::Agent, &[])
        .current()
        .iso(PerCaller),
    row("POST /rooms/:id/wiki/maintenance/renew", Rule::Feature, &[])
        .current()
        .merits(),
    row("POST /rooms/:id/wiki/runtime", Rule::SeatedAgent, &[]).current(),
    row("GET /rooms/:id/wiki/context", Rule::Feature, &[]).current(),
    // PRINCIPLES "Vault" and "The Vault informs; it does not command": "Only
    // the people who run the loca write it: its Operator, the Master and
    // Smasters. Agents, the Lead and the Wiki editor read it and do not write
    // it." The Participant is the loca's agent, its Lead and its Wiki editor.
    // A write after another caller's is stale (409), a refusal on merits.
    row("GET /rooms/:id/vault", Rule::Feature, &[]),
    row("GET /rooms/:id/vault/history", Rule::Feature, &[]),
    row("PUT /rooms/:id/vault", Rule::Manage, &[]).merits(),
    row("POST /rooms/:id/wiki/context", Rule::Feature, &[])
        .current()
        .merits(),
    row("POST /rooms/:id/wiki/context/accept", Rule::Feature, &[])
        .current()
        .merits(),
    row(
        "POST /rooms/:id/wiki/context/offer",
        Rule::AgentOrManage,
        &[],
    )
    .current()
    .merits(),
    row(
        "GET /rooms/:id/wiki/context/requests",
        Rule::AgentOrManage,
        &[],
    )
    .current(),
    // PRINCIPLES "Loca layer": "there is no separate Memory role or second
    // owner assignment. Old records are archives, not new authority." Reading
    // the archive is a feature of the loca; nobody assigns an owner, and the
    // archived owner (the loca's agent here) writes nothing.
    row("GET /rooms/:id/memory", Rule::Feature, &[]),
    row("GET /rooms/:id/memory/snapshot", Rule::Feature, &[]),
    row("GET /rooms/:id/memory/entries", Rule::Feature, &[]),
    row("PUT /rooms/:id/memory/owner", Rule::Only(&[]), &[]),
    row("PUT /rooms/:id/memory/short", Rule::Only(&[]), &[]),
    row("POST /rooms/:id/memory/entries", Rule::Only(&[]), &[]),
    // --- Direct message surfaces -----------------------------------------------
    // current behaviour, not stated in PRINCIPLES: one's own direct messages and
    // candidates; any session may ask and sees only its own.
    row("GET /birebirs", Rule::Holder, &[]).current(),
    row("GET /birebirs/candidates", Rule::Holder, &[]).current(),
    // PRINCIPLES "Birebir": "A human opens it. An agent answers in a direct message
    // but never starts one"; "a building member may open a direct message with
    // someone they sit with in a shared loca. Master and Smaster may open one
    // with any building member they could already call." The target is the
    // Participant (an agent seated with the Operator).
    row("POST /birebirs", Rule::OpenDm, &[]),
    // PRINCIPLES "Birebir": "Either of the two may explicitly call the Master
    // in".
    row("POST /birebirs/:room/master", Rule::DmPair, &[]).iso(PerRow),
    // PRINCIPLES "Birebir": "the Master sees in a separate tab that direct messages
    // exist … A Smaster does not even see that they exist"; "The Master may
    // Close a direct message it can see … or Seal it"; "Only when called in … In no
    // other way are the contents opened to a third person." (No call here.)
    row("GET /building/birebirs", Rule::MasterOnly, &[]),
    row(
        "GET /building/birebirs/:room/messages",
        Rule::DmWitness,
        &[],
    ),
    row("POST /building/birebirs/:room/close", Rule::DmMaster, &[]).iso(PerRow),
    // A seal needs a close first: the Master is turned down on its merits.
    row("POST /building/birebirs/:room/seal", Rule::DmMaster, &[])
        .iso(PerRow)
        .merits(),
];

/// The loopback-only master desk (`ADMIN_CONSOLE_PORT`) is a second router
/// with no caller identity: its door is the network bind (PRINCIPLES
/// "Identity, login, and authority": the root credential "remains in the
/// server environment"). Listed so that a new desk route is a conscious
/// decision too; not exercised by the matrix.
const CONSOLE_ROUTES: &[&str] = &[
    "GET /",
    "GET /api/state",
    "POST /api/members",
    "POST /api/invites",
    "POST /api/pairing",
    "GET /api/join-requests",
    "POST /api/join-requests/:id/approve",
    "POST /api/join-requests/:id/deny",
    "POST /api/admission-stock",
];

// ---------------------------------------------------------------------------
// Route enumeration from the router source
// ---------------------------------------------------------------------------

/// The source between two markers, with `//` comments removed so that a
/// comment can neither hide nor invent a route.
fn router_block(src: &str, start: &str, end: &str) -> String {
    let from = src
        .find(start)
        .unwrap_or_else(|| panic!("router start marker `{start}` not found in main.rs"));
    let to = src[from..]
        .find(end)
        .unwrap_or_else(|| panic!("router end marker `{end}` not found in main.rs"));
    src[from..from + to]
        .lines()
        .map(|line| {
            let mut in_string = false;
            let bytes = line.as_bytes();
            for i in 0..bytes.len() {
                match bytes[i] {
                    b'"' if i == 0 || bytes[i - 1] != b'\\' => in_string = !in_string,
                    b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
                    _ => {}
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ways to give a router paths that are not a literal `.route(` call. None
/// may appear anywhere a served router is composed.
const UNREAD_CONSTRUCTS: &[&str] = &[
    ".nest(",
    ".merge(",
    ".fallback(",
    ".method_not_allowed_fallback(",
    "route_service(",
    "nest_service(",
    "fallback_service(",
];

/// Hold one served router of `main_rs` to what the route parsers can read:
/// from `start` (its `Router::new()`) to `served` (the end of the call that
/// serves it), comments removed, there is exactly one `Router::` (no second
/// router composed in), none of `UNREAD_CONSTRUCTS` (before, between or after
/// the layers, or on the value after `with_state`), and no `.route(` after
/// `routed_until`. Shared with `principles.rs`, which reads the same chain.
pub(super) fn assert_plain_composition(
    main_rs: &str,
    start: &str,
    served: &str,
    routed_until: &str,
) {
    let region = router_block(main_rs, start, served);
    assert_eq!(
        region.matches("Router::").count(),
        1,
        "the router served from `{start}` is composed with a second `Router::`; teach the \
         route parsers in authority_matrix.rs and principles.rs to read it"
    );
    for construct in UNREAD_CONSTRUCTS {
        assert!(
            !region.contains(construct),
            "the router served from `{start}` uses `{construct}`; its paths would be served \
             without entering the authority matrix — teach authority_matrix.rs (and \
             principles.rs) to enumerate it"
        );
    }
    let until = region
        .find(routed_until)
        .unwrap_or_else(|| panic!("`{routed_until}` not found after `{start}`"));
    assert!(
        !region[until..].contains(".route("),
        "a route is added after the first `{routed_until}` of the router served from \
         `{start}`; after a .route_layer( it would bypass the membership gate"
    );
}

fn parse_routes(block: &str) -> Vec<String> {
    let mut routes = Vec::new();
    let mut rest = block;
    while let Some(at) = rest.find(".route(") {
        let open = at + ".route(".len();
        let mut depth = 1;
        let mut end = open;
        for (offset, ch) in rest[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                end = open + offset;
                break;
            }
        }
        assert_eq!(depth, 0, "unbalanced .route( in main.rs");
        let args = rest[open..end].trim_start();
        assert!(
            args.starts_with('"'),
            "a route path is not a string literal: {args}"
        );
        let close = args[1..].find('"').expect("unterminated route path") + 1;
        let path = &args[1..close];
        let methods = &args[close + 1..];
        let mut found = Vec::new();
        let bytes = methods.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &methods[start..i];
                if i < bytes.len() && bytes[i] == b'(' {
                    match ident {
                        "get" | "post" | "put" | "delete" | "patch" => {
                            found.push(ident.to_ascii_uppercase())
                        }
                        // The attachment upload lifts its body limit.
                        "layer" | "max" | "attachment_max_bytes" => {}
                        other => panic!(
                            "unknown router construct `{other}(` on route {path}; \
                             teach authority_matrix.rs to read it"
                        ),
                    }
                }
            } else {
                i += 1;
            }
        }
        assert!(!found.is_empty(), "route {path} has no method");
        for method in found {
            routes.push(format!("{method} {path}"));
        }
        rest = &rest[end..];
    }
    routes
}

/// Every `METHOD /path` of the public router. Everything from
/// `Router::new()` to the `axum::serve` call that serves it is checked: the
/// chain must add all its routes before the first `.route_layer` (axum
/// applies a route layer only to the routes added before it, so a route
/// added after the membership gate would bypass it), and nothing may compose
/// paths in any other way, after the layers included.
fn app_routes() -> Vec<String> {
    assert_plain_composition(
        MAIN_RS,
        "let app = Router::new()",
        ".expect(\"serve\")",
        ".route_layer(",
    );
    let chain = router_block(MAIN_RS, "let app = Router::new()", ".route_layer(");
    parse_routes(&chain)
}

/// Every `METHOD /path` of the loopback master desk, held the same way.
fn console_routes() -> Vec<String> {
    assert_plain_composition(
        MAIN_RS,
        "let admin_app = Router::new()",
        ".expect(\"serve admin console\")",
        ".with_state(",
    );
    parse_routes(&router_block(
        MAIN_RS,
        "let admin_app = Router::new()",
        ".with_state(",
    ))
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Resp {
    status: u16,
    body: String,
}

enum Body {
    None,
    Json(Value),
    Bytes(&'static [u8], &'static str),
}

async fn send(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Body,
) -> Resp {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut req = client.request(method, url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    req = match body {
        Body::None => req,
        Body::Json(v) => req.json(&v),
        Body::Bytes(bytes, mime) => req
            .header("content-type", mime)
            .header("x-filename", "seed.txt")
            .body(bytes.to_vec()),
    };
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    Resp { status, body }
}

fn json(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or(Value::Null)
}

fn ok(r: Resp, what: &str) -> Value {
    assert!(r.status < 300, "fixture {what}: {} {}", r.status, r.body);
    json(&r)
}

// ---------------------------------------------------------------------------
// The world: one closed Building with every kind of caller and loca
// ---------------------------------------------------------------------------

struct World {
    _guard: ServerGuard,
    _dir: tempfile::TempDir,
    port: u16,
    base: String,
    client: reqwest::Client,
    sessions: std::collections::HashMap<Who, String>,
    memberships: std::collections::HashMap<Who, String>,
    locas: std::collections::HashMap<Kind, String>,
    ghosts: std::collections::HashMap<Kind, String>,
    messages: std::collections::HashMap<Kind, u64>,
    tasks: std::collections::HashMap<Kind, u64>,
    goals: std::collections::HashMap<Kind, u64>,
    attentions: std::collections::HashMap<Kind, String>,
    attachments: std::collections::HashMap<Kind, String>,
    waits: std::collections::HashMap<Kind, String>,
    /// The management id (`ivid_`) of a Master-issued invite in `atelier`:
    /// management URLs name an invite by its id, never by the invite itself.
    quarry_invite_ref: String,
    group: String,
    spare_principal: String,
    quarry_ref: String,
    second_smaster_ref: String,
    credential_id: String,
    key_id: String,
    join_request: String,
    notification: String,
}

impl World {
    fn h(&self, who: Who) -> Vec<(&'static str, String)> {
        match self.sessions.get(&who) {
            Some(s) => vec![("x-session-token", s.clone())],
            None => Vec::new(),
        }
    }

    async fn call(&self, who: Who, method: &str, path: &str, body: Body) -> Resp {
        send(
            &self.client,
            method,
            &format!("{}{}", self.base, path),
            &self.h(who),
            body,
        )
        .await
    }

    async fn seed(&self, who: Who, method: &str, path: &str, body: Body) -> Value {
        let r = self.call(who, method, path, body).await;
        assert!(
            r.status < 300,
            "seed {method} {path}: {} {}",
            r.status,
            r.body
        );
        json(&r)
    }

    async fn build() -> World {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("matrix.db");
        let (port, guard) = spawn_server_env(
            ROOT,
            &[
                ("DB_PATH", db.to_str().unwrap().into()),
                ("REQUIRE_SESSIONS", "1".into()),
                ("REQUIRE_INVITE", "1".into()),
                ("LOCA_AGENT_ROOM", "iye".into()),
                ("RESERVED_LOCA", "iye".into()),
                ("LOCA_CARETAKERS", "loca-care".into()),
                ("LEGACY_WS_QUERY_AUTH", "0".into()),
                ("RATE_LIMIT", "1000".into()),
            ],
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        let root = vec![("x-admin-token", ROOT.to_string())];
        let mut w = World {
            _guard: guard,
            _dir: dir,
            port,
            base: base.clone(),
            client: client.clone(),
            sessions: Default::default(),
            memberships: Default::default(),
            locas: Default::default(),
            ghosts: Default::default(),
            messages: Default::default(),
            tasks: Default::default(),
            goals: Default::default(),
            attentions: Default::default(),
            attachments: Default::default(),
            waits: Default::default(),
            quarry_invite_ref: String::new(),
            group: String::new(),
            spare_principal: String::new(),
            quarry_ref: String::new(),
            second_smaster_ref: String::new(),
            credential_id: String::new(),
            key_id: String::new(),
            join_request: String::new(),
            notification: String::new(),
        };
        let url = |p: &str| format!("{base}{p}");

        // Master: a one-use pairing code minted with the root credential,
        // exchanged for a bounded session.
        let pairing = ok(
            send(
                &client,
                "POST",
                &url("/pairings?ttl_hours=1"),
                &root,
                Body::None,
            )
            .await,
            "pairing",
        );
        let master = ok(
            send(
                &client,
                "POST",
                &url("/sessions"),
                &[(
                    "x-pairing-code",
                    pairing["pairing_code"].as_str().unwrap().to_string(),
                )],
                Body::Json(serde_json::json!({"name": "", "kind": "user"})),
            )
            .await,
            "master session",
        );
        w.sessions.insert(
            Who::Master,
            master["session_token"].as_str().unwrap().to_string(),
        );

        // Smaster: a code the Master issues, signed in with.
        for name in ["deputy", "runnerup"] {
            let code = ok(
                w.call(
                    Who::Master,
                    "POST",
                    "/smasters",
                    Body::Json(serde_json::json!({"name": name})),
                )
                .await,
                "smaster",
            );
            {
                let s = ok(
                    send(
                        &client,
                        "POST",
                        &url("/sessions"),
                        &[("x-admin-token", code["token"].as_str().unwrap().to_string())],
                        Body::Json(serde_json::json!({"name": name, "kind": "user"})),
                    )
                    .await,
                    "smaster session",
                );
                let who = if name == "deputy" {
                    Who::Smaster
                } else {
                    Who::OtherSmaster
                };
                w.sessions
                    .insert(who, s["session_token"].as_str().unwrap().to_string());
            }
        }
        let smasters = ok(
            w.call(Who::Master, "GET", "/smasters", Body::None).await,
            "smasters",
        );
        w.second_smaster_ref = smasters
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "runnerup")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Building members: admitted by the Master.
        let admit = |name: &'static str, kind: &'static str| {
            let w = &w;
            async move {
                let m = ok(
                    w.call(
                        Who::Master,
                        "POST",
                        "/members",
                        Body::Json(serde_json::json!({"name": name, "kind": kind})),
                    )
                    .await,
                    name,
                );
                m["token"].as_str().unwrap().to_string()
            }
        };
        let opal = admit("opal", "user").await;
        let envoy = admit("envoy", "agent").await;
        let idler = admit("idler", "user").await;
        let care = admit("loca-care", "agent").await;
        let _lodger = admit("lodger", "agent").await;
        let _reserve = admit("reserve", "agent").await;
        let _quarry = admit("quarry", "agent").await;
        w.memberships.insert(Who::Operator, opal.clone());
        w.memberships.insert(Who::Participant, envoy.clone());
        w.memberships.insert(Who::Lobby, idler.clone());
        w.memberships.insert(Who::Caretaker, care.clone());

        let profiles = ok(
            w.call(Who::Master, "GET", "/profiles", Body::None).await,
            "profiles",
        );
        let principal = |name: &str| -> String {
            profiles
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["display_name"] == name)
                .unwrap_or_else(|| panic!("no profile {name}"))["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let opal_id = principal("opal");
        let envoy_id = principal("envoy");
        w.spare_principal = principal("reserve");
        let residents = ok(
            w.call(Who::Master, "GET", "/residents", Body::None).await,
            "residents",
        );
        w.quarry_ref = residents
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "quarry")
            .unwrap()["membership_id"]
            .as_str()
            .unwrap()
            .to_string();

        // An ordinary loca, explicitly created; the human Operator appointed.
        ok(
            w.call(
                Who::Master,
                "POST",
                "/rooms",
                Body::Json(serde_json::json!({
                    "name": "atelier",
                    "description": IDENTITY_MARKS[0].1[0],
                    "labels": [IDENTITY_MARKS[0].1[1]],
                    "icon": "compass",
                    "color": "green",
                })),
            )
            .await,
            "create atelier",
        );
        w.locas.insert(Kind::Ordinary, "atelier".into());
        let mut quarry_invite_ref = String::new();
        for name in ["opal", "envoy", "quarry"] {
            let d = ok(
                w.call(
                    Who::Master,
                    "POST",
                    "/rooms/atelier/invites",
                    Body::Json(serde_json::json!({"name": name})),
                )
                .await,
                "seat in atelier",
            );
            quarry_invite_ref = d["id"].as_str().unwrap().to_string();
        }
        let lodger_invite = ok(
            w.call(
                Who::Master,
                "POST",
                "/rooms/atelier/invites",
                Body::Json(serde_json::json!({"name": "lodger"})),
            )
            .await,
            "lodger invite",
        )["token"]
            .as_str()
            .unwrap()
            .to_string();
        ok(
            w.call(
                Who::Master,
                "POST",
                "/rooms/atelier/operators",
                Body::Json(serde_json::json!({"principal_id": opal_id})),
            )
            .await,
            "appoint opal",
        );

        // Sessions for members, taken with their own membership key; the
        // invite holder takes one with nothing but the invite.
        let session =
            |header: &'static str, key: String, name: &'static str, kind: &'static str| {
                let client = client.clone();
                let url = url("/sessions");
                async move {
                    let s = ok(
                        send(
                            &client,
                            "POST",
                            &url,
                            &[(header, key)],
                            Body::Json(serde_json::json!({"name": name, "kind": kind})),
                        )
                        .await,
                        name,
                    );
                    s["session_token"].as_str().unwrap().to_string()
                }
            };
        w.sessions.insert(
            Who::Operator,
            session("x-room-token", opal.clone(), "opal", "user").await,
        );
        w.sessions.insert(
            Who::Participant,
            session("x-room-token", envoy.clone(), "envoy", "agent").await,
        );
        w.sessions.insert(
            Who::Lobby,
            session("x-room-token", idler.clone(), "idler", "user").await,
        );
        w.sessions.insert(
            Who::Invite,
            session("x-room-token", lodger_invite, "lodger", "agent").await,
        );

        // A group with its own loca, founded by the Smaster; the participant
        // and a spare agent are its members.
        let group = ok(
            w.call(
                Who::Smaster,
                "POST",
                "/groups",
                Body::Json(serde_json::json!({"name": "squad"})),
            )
            .await,
            "group",
        );
        w.group = group["id"].as_str().unwrap().to_string();
        for p in [&envoy_id, &w.spare_principal.clone()] {
            ok(
                w.call(
                    Who::Smaster,
                    "PUT",
                    &format!("/groups/{}/members/{p}", w.group),
                    Body::None,
                )
                .await,
                "group member",
            );
        }
        ok(
            w.call(
                Who::Smaster,
                "POST",
                "/rooms",
                Body::Json(serde_json::json!({
                    "name": "squad-atelier",
                    "group": w.group,
                    "description": IDENTITY_MARKS[1].1[0],
                    "labels": [IDENTITY_MARKS[1].1[1]],
                    "icon": "anchor",
                    "color": "vermillion",
                })),
            )
            .await,
            "group loca",
        );
        w.locas.insert(Kind::Group, "squad-atelier".into());
        // A title is given after opening, from the identity panel.
        for (kind, marks) in IDENTITY_MARKS {
            let path = format!("/rooms/{}/settings", w.locas[kind]);
            ok(
                w.call(
                    Who::Master,
                    "PUT",
                    &path,
                    Body::Json(serde_json::json!({"title": marks[4]})),
                )
                .await,
                "title",
            );
        }
        // The identities the matrix watches for are really there, so a
        // caller who may enter reads them (and the scan is not vacuous).
        for (kind, marks) in IDENTITY_MARKS {
            let path = format!("/rooms/{}/settings", w.locas[kind]);
            let settings = ok(
                w.call(Who::Master, "GET", &path, Body::None).await,
                "identity",
            );
            assert_eq!(settings["description"], marks[0], "{settings}");
            assert_eq!(settings["labels"][0], marks[1], "{settings}");
            assert_eq!(settings["title"], marks[4], "{settings}");
        }

        // A direct message: the human Operator opens one with the agent it sits with.
        let bb = ok(
            w.call(
                Who::Operator,
                "POST",
                "/birebirs",
                Body::Json(serde_json::json!({"name": "envoy"})),
            )
            .await,
            "dm",
        );
        w.locas.insert(
            Kind::Dm,
            bb["birebir"]["room"].as_str().unwrap().to_string(),
        );

        // Care: the caretaker sits there through its own invite.
        w.locas.insert(Kind::CareLoca, "iye".into());
        ok(
            w.call(
                Who::Master,
                "POST",
                "/rooms/iye/invites",
                Body::Json(serde_json::json!({"name": "loca-care"})),
            )
            .await,
            "caretaker invite",
        );
        w.sessions.insert(
            Who::Caretaker,
            session("x-room-token", care.clone(), "loca-care", "agent").await,
        );

        // Ghosts: ids of the same shape that name no loca.
        w.ghosts.insert(Kind::Ordinary, "ghost-atelier".into());
        w.ghosts.insert(Kind::Group, "ghost-squad".into());
        let bb_room = w.locas[&Kind::Dm].clone();
        let ghost_bb = format!("bb-{}", "0".repeat(bb_room.len() - 3));
        w.ghosts.insert(Kind::Dm, ghost_bb);
        w.ghosts.insert(Kind::CareLoca, "ghost-iye".into());

        // Seeds in every loca, made by someone who may make them, so that a
        // caller who may act reaches a real record rather than a not-found.
        for kind in KINDS {
            let loca = w.locas[&kind].clone();
            let (author, agent, agent_name) = match kind {
                Kind::Dm => (Who::Operator, Who::Participant, "envoy"),
                Kind::CareLoca => (Who::Master, Who::Caretaker, "loca-care"),
                _ => (Who::Master, Who::Participant, "envoy"),
            };
            let at = |p: &str| format!("/rooms/{loca}{p}");
            let m = w
                .seed(
                    author,
                    "POST",
                    &at("/messages"),
                    Body::Json(serde_json::json!({
                        "sender": "x", "sender_type": "user", "text": "seed"
                    })),
                )
                .await;
            w.messages.insert(kind, m["id"].as_u64().unwrap());
            let a = w
                .seed(
                    author,
                    "POST",
                    &at("/attachments"),
                    Body::Bytes(b"seed attachment\n", "text/plain"),
                )
                .await;
            let att = a["id"].as_str().unwrap().to_string();
            w.seed(
                author,
                "POST",
                &at("/messages"),
                Body::Json(serde_json::json!({
                    "sender": "x", "sender_type": "user", "text": "file", "attachments": [att]
                })),
            )
            .await;
            w.attachments.insert(kind, att);
            if kind == Kind::Dm {
                continue;
            }
            // The Vault is not in the legacy set: switch it on, so its rows
            // meet the door and the management check, not the fence.
            w.seed(Who::Master, "PUT", &at("/extensions/vault"), Body::None)
                .await;
            w.seed(
                author,
                "POST",
                &at("/lead"),
                Body::Json(serde_json::json!({"lead": agent_name})),
            )
            .await;
            w.seed(
                author,
                "POST",
                &at("/notes"),
                Body::Json(serde_json::json!({
                    "key": "seed", "title": "Seed", "body": "seed", "by": "x"
                })),
            )
            .await;
            let t = w
                .seed(
                    author,
                    "POST",
                    &at("/tasks"),
                    Body::Json(serde_json::json!({"title": "seed", "by": "x"})),
                )
                .await;
            w.tasks.insert(kind, t["id"].as_u64().unwrap());
            let g = w
                .seed(
                    author,
                    "POST",
                    &at("/goals"),
                    Body::Json(serde_json::json!({"outcome": "seed", "by": "x"})),
                )
                .await;
            w.goals.insert(kind, g["id"].as_u64().unwrap());
            let att = w
                .seed(
                    author,
                    "POST",
                    &at("/attentions"),
                    Body::Json(serde_json::json!({"subject": "seed", "by": "x"})),
                )
                .await;
            w.attentions.insert(
                kind,
                att["id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| att["id"].to_string()),
            );
            w.seed(
                agent,
                "POST",
                &at("/waits"),
                Body::Json(serde_json::json!({
                    "waiting_for": "opal", "reason": "seed", "by": "x"
                })),
            )
            .await;
            w.waits.insert(kind, agent_name.into());
            w.seed(
                agent,
                "POST",
                &at("/wiki/runtime"),
                Body::Json(serde_json::json!({"runtime": "codex"})),
            )
            .await;
            w.seed(
                author,
                "PUT",
                &at("/wiki/config"),
                Body::Json(serde_json::json!({
                    "editor": agent_name, "enabled": true, "interval_messages": 20
                })),
            )
            .await;
            w.seed(
                author,
                "PUT",
                &at("/wiki/pages/seed"),
                Body::Json(serde_json::json!({
                    "expected_revision": 0, "title": "Seed", "body": "seed"
                })),
            )
            .await;
            // An archived memory record, as an upgraded Building holds one:
            // its owner was the loca's agent, bound to its principal, before
            // the Memory role was retired. The memory routes have a record to
            // read, and the archived owner is the caller most likely to be
            // let write.
            let owner_id = principal(agent_name);
            rusqlite::Connection::open(&db)
                .unwrap()
                .execute(
                    "INSERT INTO loca_memory
                     (room, owner, owner_principal_id, short, long, short_updated_at,
                      long_updated_at, version)
                     VALUES (?1, ?2, ?3, 'archived state', 'archived decision',
                             1700000000000, 1700000000000, 1)",
                    rusqlite::params![loca, agent_name, owner_id],
                )
                .unwrap();
            rusqlite::Connection::open(&db)
                .unwrap()
                .execute(
                    "INSERT INTO loca_memory_entries (room, text, decided_by, decided_at)
                     VALUES (?1, 'archived decision', ?2, 1700000000000)",
                    rusqlite::params![loca, agent_name],
                )
                .unwrap();
        }
        w.quarry_invite_ref = quarry_invite_ref;
        // A code in the Master's key ledger: the second Smaster's.
        let keys = w.seed(Who::Master, "GET", "/keys", Body::None).await;
        w.key_id = keys
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == "smaster" && k["for_name"] == "runnerup")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // A profile credential of the participant's own.
        let cred = ok(
            w.call(
                Who::Participant,
                "POST",
                "/profile/credentials",
                Body::Json(serde_json::json!({"label": "spare"})),
            )
            .await,
            "credential",
        );
        w.credential_id = cred["credential"]["id"]
            .as_str()
            .or(cred["id"].as_str())
            .unwrap_or("")
            .to_string();

        // A join request from outside.
        let jr = ok(
            send(
                &client,
                "POST",
                &url("/join-requests"),
                &[],
                Body::Json(serde_json::json!({"name": "interloper", "kind": "agent"})),
            )
            .await,
            "join request",
        );
        w.join_request = jr["request_id"].as_str().unwrap().to_string();

        // A Building notification from the Master to everyone.
        let n = ok(
            w.call(
                Who::Master,
                "POST",
                "/notifications",
                Body::Json(serde_json::json!({
                    "title": "seed", "body": "seed", "audience": {"all": true}
                })),
            )
            .await,
            "notification",
        );
        w.notification = n["id"]
            .as_str()
            .map(str::to_string)
            .or(n["id"].as_u64().map(|v| v.to_string()))
            .or(n["notification"]["id"].as_str().map(str::to_string))
            .unwrap_or_default();
        w
    }
}

// ---------------------------------------------------------------------------
// Concrete requests
// ---------------------------------------------------------------------------

fn path_of(route: &str) -> &str {
    route.split_once(' ').unwrap().1
}

/// Routes that name a loca (in the path, or `/ws?room=`): called once per
/// loca kind.
fn names_loca(route: &str) -> bool {
    let path = path_of(route);
    path.starts_with("/rooms/:id")
        || path.starts_with("/birebirs/:room")
        || path.starts_with("/building/birebirs/:room")
        || path == "/ws"
}

/// A short name per caller, so that a create made by one caller does not
/// collide with the same create made by the next.
fn tag(who: Who) -> &'static str {
    match who {
        Who::Outsider => "x",
        Who::Invite => "d",
        Who::Caretaker => "c",
        Who::Lobby => "l",
        Who::Participant => "p",
        Who::Operator => "o",
        Who::OtherSmaster => "t",
        Who::Smaster => "s",
        Who::Master => "m",
    }
}

/// The name the server knows each caller by.
fn display_name(who: Who) -> &'static str {
    match who {
        Who::Outsider => "nobody",
        Who::Invite => "lodger",
        Who::Caretaker => "loca-care",
        Who::Lobby => "idler",
        Who::Participant => "envoy",
        Who::Operator => "opal",
        Who::OtherSmaster => "runnerup",
        Who::Smaster => "deputy",
        Who::Master => "Master",
    }
}

/// Fill a route's path parameters for `kind`. A ghost replaces the route's
/// first parameter (the loca, group, request, …) with an id of the same
/// shape that names nothing.
fn concrete_path(w: &World, route: &str, kind: Option<Kind>, ghost: bool) -> String {
    let path = path_of(route);
    let k = kind.unwrap_or(Kind::Ordinary);
    let loca = if ghost {
        w.ghosts[&k].clone()
    } else {
        w.locas[&k].clone()
    };
    let segs: Vec<&str> = path.split('/').collect();
    let mut out = Vec::new();
    let mut first_param = true;
    for seg in &segs {
        let is_param = seg.starts_with(':');
        let ghost_here = is_param && std::mem::take(&mut first_param) && ghost;
        let value = match *seg {
            ":id" if segs[1] == "rooms" => loca.clone(),
            ":room" => loca.clone(),
            _ if ghost_here => "ghost-00000000000000000000000000000000".to_string(),
            ":id" if segs[1] == "groups" => w.group.clone(),
            ":id" if segs[1] == "join-requests" => w.join_request.clone(),
            ":id" if segs[1] == "notifications" => w.notification.clone(),
            ":id" if segs[1] == "keys" => w.key_id.clone(),
            ":principal_id" => w.spare_principal.clone(),
            ":credential_id" => w.credential_id.clone(),
            ":token" if segs[1] == "members" => w.quarry_ref.clone(),
            ":token" if segs[1] == "smasters" => w.second_smaster_ref.clone(),
            ":token" => w.quarry_invite_ref.clone(),
            ":name" if segs[1] == "assets" => "app.js".into(),
            ":name" if segs[1] == "downloads" => "loca".into(),
            ":name" => w.waits.get(&k).cloned().unwrap_or_else(|| "envoy".into()),
            ":att_id" => w.attachments[&k].clone(),
            ":message_id" => w.messages[&k].to_string(),
            ":ext" => "notes".into(),
            ":slug" | ":key" => "seed".into(),
            ":tid" => w.tasks.get(&k).copied().unwrap_or(1).to_string(),
            ":gid" => w.goals.get(&k).copied().unwrap_or(1).to_string(),
            ":attention_id" => w
                .attentions
                .get(&k)
                .cloned()
                .unwrap_or_else(|| "none".into()),
            ":signal_id" => "no-such-signal".into(),
            s if s.starts_with(':') => panic!("unresolved path parameter {s} in {route}"),
            s => s.to_string(),
        };
        out.push(value);
    }
    let mut out = out.join("/");
    if route == "GET /rooms/:id/search" {
        out.push_str("?q=seed");
    }
    out
}

/// A request body that would succeed for a caller who may act.
fn body_for(route: &str, w: &World, who: Who, kind: Option<Kind>) -> Body {
    let t = tag(who);
    let lead = match kind {
        Some(Kind::CareLoca) => "loca-care",
        _ => "envoy",
    };
    let j = Body::Json;
    match route {
        "POST /sessions" => j(serde_json::json!({"name": "probe", "kind": "agent"})),
        "POST /admission-stock" => j(serde_json::json!({"count": 1})),
        "POST /join-requests" => {
            j(serde_json::json!({"name": format!("aday-{t}"), "kind": "agent"}))
        }
        "PUT /profile" => j(serde_json::json!({"display_name": format!("yeni-{t}")})),
        "POST /groups" => j(serde_json::json!({"name": format!("crew-{t}")})),
        "PATCH /groups/:id" => j(serde_json::json!({"name": format!("squad-{t}")})),
        "PUT /rooms/:id/group" => j(serde_json::json!({"group": w.group})),
        "POST /notifications" => j(serde_json::json!({
            "title": "probe", "body": "probe", "audience": {"all": true}
        })),
        "PUT /profile/preferences" => j(serde_json::json!({"theme": "dark"})),
        "PUT /profile/card" => j(serde_json::json!({"icon": "leaf"})),
        "POST /profile/credentials" => j(serde_json::json!({"label": format!("probe-{t}")})),
        "POST /members" => j(serde_json::json!({"name": format!("enrollee-{t}"), "kind": "agent"})),
        "POST /runtime/health" => j(serde_json::json!({"wake": "IDLE", "ack": "IDLE"})),
        "POST /members/:token/suspension" => j(serde_json::json!({"suspended": true})),
        "POST /rooms/:id/call" | "POST /rooms/:id/invites" => {
            j(serde_json::json!({"name": "reserve"}))
        }
        "POST /smasters" => j(serde_json::json!({"name": format!("sm-{t}")})),
        "PUT /smasters/:token" => j(serde_json::json!({"name": format!("runnerup-{t}")})),
        "POST /keys" => j(serde_json::json!({"kind": "admission", "count": 1})),
        "POST /rooms" => j(serde_json::json!({"name": format!("parlor-{t}")})),
        "POST /rooms/:id/messages" => j(serde_json::json!({
            "sender": "x", "sender_type": "user", "text": "probe"
        })),
        "POST /rooms/:id/attachments" => Body::Bytes(b"probe attachment\n", "text/plain"),
        "POST /rooms/:id/messages/:message_id/reactions" => {
            j(serde_json::json!({"emoji": "♥", "active": true}))
        }
        "PUT /rooms/:id/mode" => j(serde_json::json!({"mode": {"mode": "free"}})),
        "POST /rooms/:id/lead" => j(serde_json::json!({"lead": lead})),
        // An agent: an allowed caller is turned down on its merits.
        "POST /rooms/:id/operators" => j(serde_json::json!({"principal_id": w.spare_principal})),
        "PUT /rooms/:id/settings" => j(serde_json::json!({"rate_limit": 1000})),
        "PUT /rooms/:id/settings {identity}" => j(serde_json::json!({
            "description": format!("set by {t}"), "labels": ["probe"]
        })),
        "PUT /rooms/:id/extensions" => j(serde_json::json!({
            "enabled": ["notes", "focus", "journal", "wiki", "vault"]
        })),
        "PUT /rooms/:id/vault" => j(serde_json::json!({
            "body": format!("rules by {t}"), "base_rev": 0
        })),
        "POST /rooms/:id/moderate" => j(serde_json::json!({"action": "mute", "name": "quarry"})),
        "POST /rooms/:id/notes" => j(serde_json::json!({
            "key": format!("probe-{t}"), "title": "p", "body": "p", "by": "x"
        })),
        "PUT /rooms/:id/notes/:key" => j(serde_json::json!({"body": format!("by {t}"), "by": "x"})),
        "PUT /rooms/:id/wiki/pages/:slug" => j(serde_json::json!({
            "expected_revision": 1, "title": "Seed", "body": format!("by {t}")
        })),
        "DELETE /rooms/:id/wiki/pages/:slug" => {
            j(serde_json::json!({"expected_revision": 1, "reason": "probe"}))
        }
        "PUT /rooms/:id/wiki/config" => j(serde_json::json!({
            "editor": lead, "enabled": true, "interval_messages": 20
        })),
        "POST /rooms/:id/wiki/review" => j(serde_json::json!({
            "expected_revision": 1, "reviewed_through": 0, "reason": "probe", "pages": []
        })),
        "POST /rooms/:id/wiki/runtime" => j(serde_json::json!({"runtime": "codex"})),
        "POST /rooms/:id/wiki/context" => {
            j(serde_json::json!({"slugs": ["seed"], "reason": "probe"}))
        }
        "POST /rooms/:id/wiki/context/accept" => j(serde_json::json!({
            "context_id": "none", "packet_id": "none", "revision": 1, "selection_version": 1
        })),
        "POST /rooms/:id/wiki/context/offer" => j(serde_json::json!({
            "target": lead, "slugs": ["seed"], "reason": "probe"
        })),
        "POST /rooms/:id/wiki/maintenance/finish" => j(serde_json::json!({
            "lease": {"batch_id": 1, "epoch": 1},
            "review": {"expected_revision": 1, "reviewed_through": 0, "reason": "p", "pages": []}
        })),
        "POST /rooms/:id/wiki/maintenance/renew" => {
            j(serde_json::json!({"batch_id": 1, "epoch": 1}))
        }
        "PUT /rooms/:id/memory/owner" => j(serde_json::json!({"owner": lead})),
        "PUT /rooms/:id/memory/short" | "POST /rooms/:id/memory/entries" => {
            j(serde_json::json!({"text": format!("by {t}")}))
        }
        "POST /rooms/:id/journal" => j(serde_json::json!({"text": format!("by {t}")})),
        "POST /rooms/:id/tasks" => j(serde_json::json!({"title": format!("by {t}"), "by": "x"})),
        "PATCH /rooms/:id/tasks/:tid" => j(serde_json::json!({
            "status": "taken", "assigned_to": display_name(who), "by": "x"
        })),
        "POST /rooms/:id/goals" => j(serde_json::json!({"outcome": "probe", "by": "x"})),
        "PATCH /rooms/:id/goals/:gid" => {
            j(serde_json::json!({"checkpoint": format!("by {t}"), "by": "x"}))
        }
        "POST /rooms/:id/attentions" => j(serde_json::json!({"subject": "probe", "by": "x"})),
        "POST /rooms/:id/attentions/:attention_id/claim"
        | "POST /rooms/:id/attentions/:attention_id/resolve"
        | "POST /rooms/:id/care/:signal_id/ack"
        | "DELETE /rooms/:id/waits/:name" => j(serde_json::json!({"by": "x"})),
        "POST /rooms/:id/waits" => j(serde_json::json!({
            "waiting_for": "opal", "reason": "probe", "by": "x"
        })),
        "POST /birebirs" => j(serde_json::json!({"name": "envoy"})),
        "POST /birebirs/:room/master" => j(serde_json::json!({"action": "call"})),
        "POST /building/birebirs/:room/close" => j(serde_json::json!({})),
        _ => Body::None,
    }
}

/// One call: `route` by `who` against `kind` (or its ghost).
async fn probe(
    w: &World,
    route: &str,
    body: &str,
    who: Who,
    kind: Option<Kind>,
    ghost: bool,
) -> Resp {
    match route {
        "GET /ws" => return ws_probe(w, who, kind.unwrap(), ghost).await,
        "GET /lobby/ws" => return lobby_probe(w, who).await,
        _ => {}
    }
    let (method, _) = route.split_once(' ').unwrap();
    let path = concrete_path(w, route, kind, ghost);
    w.call(who, method, &path, body_for(body, w, who, kind))
        .await
}

async fn handshake(url: String, protocols: Vec<String>) -> Resp {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&protocols.join(", ")).unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok((mut ws, response)) => {
            let _ = ws.close(None).await;
            Resp {
                status: response.status().as_u16(),
                body: String::new(),
            }
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Resp {
            status: response.status().as_u16(),
            body: response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default(),
        },
        Err(other) => panic!("websocket handshake failed outside HTTP: {other}"),
    }
}

/// The loca door over WebSocket: the session rides in the subprotocol.
async fn ws_probe(w: &World, who: Who, kind: Kind, ghost: bool) -> Resp {
    let room = if ghost {
        w.ghosts[&kind].clone()
    } else {
        w.locas[&kind].clone()
    };
    let mut protocols = vec!["loca.v1".to_string()];
    if let Some(s) = w.sessions.get(&who) {
        protocols.push(format!("loca.session.{s}"));
    }
    handshake(
        format!(
            "ws://127.0.0.1:{}/ws?room={room}&name=probe&type=agent",
            w.port
        ),
        protocols,
    )
    .await
}

/// The Lobby door: a membership key, or for a caller without one the
/// session it holds.
async fn lobby_probe(w: &World, who: Who) -> Resp {
    let mut protocols = vec!["loca.v1".to_string()];
    if let Some(m) = w.memberships.get(&who).or(w.sessions.get(&who)) {
        protocols.push(format!("loca.membership.{m}"));
    }
    handshake(format!("ws://127.0.0.1:{}/lobby/ws", w.port), protocols).await
}

// ---------------------------------------------------------------------------
// Running and judging
// ---------------------------------------------------------------------------

/// The identity of the ordinary loca and of the group loca in this world
/// (PRINCIPLES "A loca's identity"). Whoever may not enter a loca learns
/// nothing of it: no answer of any route, to any caller outside its door,
/// carries its title, description, labels, icon or colour.
const IDENTITY_MARKS: &[(Kind, &[&str])] = &[
    (
        Kind::Ordinary,
        &[
            "atelier-purpose-7f3k",
            "atelier-label-q2",
            "\"icon\":\"compass\"",
            "\"color\":\"green\"",
            "atelier-title-m4",
        ],
    ),
    (
        Kind::Group,
        &[
            "squad-purpose-9z1w",
            "squad-label-x7",
            "\"icon\":\"anchor\"",
            "\"color\":\"vermillion\"",
            "squad-title-p8",
        ],
    ),
];

/// A mark of a loca `who` may not enter, found in an answer it was given.
fn identity_leak(who: Who, seen: &Resp) -> Option<String> {
    IDENTITY_MARKS.iter().find_map(|(kind, marks)| {
        if door(who, *kind) == Door::In {
            return None;
        }
        marks
            .iter()
            .find(|mark| seen.body.contains(*mark))
            .map(|mark| format!("{kind:?}'s identity ({mark})"))
    })
}

/// The expectation for one cell, and where it comes from.
fn expectation(row: &Row, who: Who, kind: Option<Kind>) -> (Want, String) {
    if let Some(cell) = row
        .cells
        .iter()
        .find(|c| c.who == who && (c.kind.is_none() || c.kind == kind))
    {
        return match cell.why {
            Why::Current(note) => (cell.want, format!("current behaviour: {note}")),
            Why::Gap(g) => (
                cell.want,
                format!(
                    "{}: {} If the server now follows the principle, remove the cell \
                     (principle: {}; severity: {})",
                    g.id, g.happens, g.principle, g.severity
                ),
            ),
        };
    }
    let (mut want, gap) = want_for(row.rule, row.route, who, kind);
    if row.absent && want == Want::Allow {
        want = Want::Absent;
    }
    let basis = match (gap, row.basis) {
        (Some(g), _) => format!(
            "{}: {} (principle: {}; severity: {})",
            g.id, g.happens, g.principle, g.severity
        ),
        (None, Basis::Doc) => format!("{:?}", row.rule),
        (None, Basis::Current) => format!("{:?} (current behaviour)", row.rule),
    };
    (want, basis)
}

fn judge(row: &Row, want: Want, seen: &Resp, ghost: Option<&Resp>) -> bool {
    let s = seen.status;
    let refused = matches!(s, 401 | 403 | 404);
    let same_as_ghost = ghost.is_some_and(|g| g.status == s && g.body == seen.body);
    let merits = matches!(s, 400 | 409 | 422);
    match want {
        Want::Allow => (200..300).contains(&s) || s == 101 || (row.merits && merits),
        Want::Refuse => refused,
        Want::Hidden => refused && same_as_ghost,
        Want::Absent => s == 404 && !same_as_ghost,
        Want::Rejected => merits,
    }
}

fn cell_label(who: Who, kind: Option<Kind>) -> String {
    match kind {
        Some(k) => format!("{who:?}@{k:?}"),
        None => format!("{who:?}"),
    }
}

/// Call `row` by every caller in `callers` against every loca kind it
/// names, and return one line per cell that does not meet its expectation.
async fn run_row(w: &World, row: &Row, callers: &[Who]) -> (Vec<String>, String) {
    let kinds: Vec<Option<Kind>> = if names_loca(row.route) {
        KINDS.iter().copied().map(Some).collect()
    } else {
        vec![None]
    };
    let has_ghost = row.route.contains(':') || names_loca(row.route);
    let mut failures = Vec::new();
    let mut observed = String::new();
    for kind in kinds {
        if let Some(k) = kind {
            observed.push_str(&format!(" |{k:?}"));
        }
        for &who in callers {
            let seen = probe(w, row.route, row.name(), who, kind, false).await;
            let (want, basis) = expectation(row, who, kind);
            let ghost =
                if has_ghost && (matches!(seen.status, 401 | 403 | 404) || want == Want::Hidden) {
                    Some(probe(w, row.route, row.name(), who, kind, true).await)
                } else {
                    None
                };
            let hidden = ghost
                .as_ref()
                .is_some_and(|g| g.status == seen.status && g.body == seen.body);
            observed.push_str(&format!(
                " {}{}{}",
                tag(who).to_ascii_uppercase(),
                seen.status,
                if hidden { "=" } else { "" }
            ));
            // The router must serve the route: a 405 means the method is not
            // there, and only axum's own fallback answers 404 with no body.
            let unrouted = seen.status == 405
                || (who == Who::Outsider && seen.status == 404 && seen.body.is_empty());
            if let Some(leak) = identity_leak(who, &seen) {
                failures.push(format!(
                    "{} {}: {leak} reached a caller who may not enter that loca",
                    row.name(),
                    cell_label(who, kind)
                ));
            }
            if unrouted || !judge(row, want, &seen, ghost.as_ref()) {
                failures.push(format!(
                    "{} {}: want {want:?} [{basis}], got {} {:?}{}",
                    row.name(),
                    cell_label(who, kind),
                    seen.status,
                    seen.body.chars().take(120).collect::<String>(),
                    match &ghost {
                        Some(g) => format!(
                            "; nonexistent id answers {} {:?}",
                            g.status,
                            g.body.chars().take(80).collect::<String>()
                        ),
                        None => String::new(),
                    }
                ));
            }
        }
    }
    (failures, observed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A route the router serves without a row (or a row the router no longer
/// serves) fails here, before any server starts.
#[test]
fn every_router_route_has_exactly_one_row() {
    let routes = app_routes();
    let listed: Vec<&str> = ROWS.iter().map(|r| r.route).collect();
    let mut problems = Vec::new();
    for route in &routes {
        match listed.iter().filter(|r| **r == route.as_str()).count() {
            0 => problems.push(format!(
                "{route}: served by the router but has no row in authority_matrix.rs ROWS; \
                 add one stating who may call it (cite PRINCIPLES/DESIGN, or mark it \
                 current behaviour)"
            )),
            1 => {}
            n => problems.push(format!("{route}: {n} rows")),
        }
    }
    for row in listed {
        if !routes.iter().any(|r| r == row) {
            problems.push(format!("{row}: has a row but the router does not serve it"));
        }
    }
    let console = console_routes();
    for route in &console {
        if !CONSOLE_ROUTES.contains(&route.as_str()) {
            problems.push(format!(
                "master desk {route}: served but not listed in CONSOLE_ROUTES"
            ));
        }
    }
    for route in CONSOLE_ROUTES {
        if !console.iter().any(|r| r == route) {
            problems.push(format!("master desk {route}: listed but not served"));
        }
    }
    assert!(
        problems.is_empty(),
        "router and authority table differ:\n{}",
        problems.join("\n")
    );
    // A loca rule on a route that names no loca, or a cell for a caller the
    // rule already gives, is a mistake in the table.
    for variant in BODY_VARIANTS {
        assert!(
            ROWS.iter().any(|row| row.route == variant.route),
            "{}: a body variant of a route with no row",
            variant.name()
        );
    }
    for row in ROWS.iter().chain(BODY_VARIANTS) {
        let kinds: Vec<Option<Kind>> = if names_loca(row.route) {
            KINDS.iter().copied().map(Some).collect()
        } else {
            vec![None]
        };
        for kind in &kinds {
            for who in CALLERS {
                let _ = want_for(row.rule, row.route, who, *kind);
            }
        }
        for cell in row.cells {
            let applies = kinds.iter().any(|k| cell.kind.is_none() || cell.kind == *k);
            assert!(
                applies,
                "{}: cell {:?} names a loca kind the route has not",
                row.route, cell
            );
            let ruled = kinds
                .iter()
                .filter(|k| cell.kind.is_none() || cell.kind == **k)
                .all(|k| want_for(row.rule, row.route, cell.who, *k).0 == cell.want);
            assert!(
                !ruled || matches!(cell.why, Why::Gap(_)),
                "{}: cell {:?} only repeats the rule",
                row.route,
                cell
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authority_matrix() {
    let started = std::time::Instant::now();
    every_router_route_has_exactly_one_row();
    let filter = std::env::var("AUTHORITY_MATRIX_DUMP").ok();
    let rows: Vec<&'static Row> = ROWS
        .iter()
        .chain(BODY_VARIANTS)
        .filter(|r| filter.as_deref().is_none_or(|f| r.name().contains(f)))
        .collect();

    // Rows that change what other rows rely on get worlds of their own; the
    // rest share one. At most a few servers boot at once.
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(6));
    let mut jobs = Vec::new();
    let shared: Vec<&'static Row> = rows
        .iter()
        .copied()
        .filter(|r| r.iso == Iso::Shared)
        .collect();
    {
        let permits = permits.clone();
        jobs.push(tokio::spawn(async move {
            let _permit = permits.acquire().await.unwrap();
            let w = World::build().await;
            let mut out = Vec::new();
            for row in shared {
                let (fails, line) = run_row(&w, row, &CALLERS).await;
                out.push((row.name(), fails, line));
            }
            out
        }));
    }
    for row in rows.iter().copied().filter(|r| r.iso != Iso::Shared) {
        // A row that consumes its target gives each caller who may act a
        // world of its own; the callers it refuses share one.
        let groups: Vec<Vec<Who>> = match row.iso {
            Iso::Caller => {
                let acts = |who: &Who| {
                    let kinds: Vec<Option<Kind>> = if names_loca(row.route) {
                        KINDS.iter().copied().map(Some).collect()
                    } else {
                        vec![None]
                    };
                    kinds.into_iter().any(|k| {
                        !matches!(expectation(row, *who, k).0, Want::Refuse | Want::Hidden)
                    })
                };
                let mut groups: Vec<Vec<Who>> =
                    vec![CALLERS.iter().copied().filter(|w| !acts(w)).collect()];
                groups.extend(CALLERS.iter().copied().filter(acts).map(|w| vec![w]));
                groups
            }
            _ => vec![CALLERS.to_vec()],
        };
        for callers in groups {
            let permits = permits.clone();
            jobs.push(tokio::spawn(async move {
                let _permit = permits.acquire().await.unwrap();
                let w = World::build().await;
                let (fails, line) = run_row(&w, row, &callers).await;
                vec![(row.name(), fails, line)]
            }));
        }
    }
    let mut failures = Vec::new();
    let mut failed: std::collections::BTreeSet<&str> = Default::default();
    let mut observed: std::collections::BTreeMap<&str, String> = Default::default();
    for job in jobs {
        for (route, fails, line) in job.await.unwrap() {
            observed.entry(route).or_default().push_str(&line);
            if !fails.is_empty() {
                failed.insert(route);
            }
            failures.extend(fails);
        }
    }
    let lines = |routes: Vec<&str>| {
        routes
            .into_iter()
            .map(|r| format!("{r}:{}", observed[r]))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if filter.is_some() {
        eprintln!("{}", lines(observed.keys().copied().collect()));
        eprintln!("elapsed {:?}", started.elapsed());
        return;
    }
    if !failures.is_empty() {
        panic!(
            "{} cell(s) do not match the authority table:\n{}\n\nobserved (=: same answer as \
             a nonexistent id):\n{}",
            failures.len(),
            failures.join("\n"),
            lines(failed.into_iter().collect())
        );
    }
    eprintln!(
        "authority matrix: {} rows in {:?}",
        ROWS.len(),
        started.elapsed()
    );
}
