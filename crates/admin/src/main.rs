//! loca-admin — the master's console: hand out invites, take them back.
//!
//! An invite is the right to enter one loca. Only the master issues one, and the
//! master's key lives in the building's own `.env`. Until now that meant
//! `ssh` + `invite.sh`, which locks invitation to whoever holds shell access.
//! This is the same authority in a window you can hand to a person.
//!
//! Single binary, no runtime, no install: settings live in `~/.loca/admin.toml`
//! (mode 600) so a second machine only needs the file and the address.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{execute, terminal};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};

mod api;
mod settings;

use api::{Api, Invite, Reissued};
use settings::Settings;

/// Which screen is in front.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Invites,
    Issue,
    Settings,
}

impl Tab {
    fn all() -> [Tab; 3] {
        [Tab::Invites, Tab::Issue, Tab::Settings]
    }
    fn title(self) -> &'static str {
        match self {
            Tab::Invites => "invites",
            Tab::Issue => "issue",
            Tab::Settings => "settings",
        }
    }
}

/// Where typing goes on the issue screen.
#[derive(Clone, Copy, PartialEq)]
enum Field {
    Loca,
    Name,
}

/// Where typing goes on the settings screen.
#[derive(Clone, Copy, PartialEq)]
enum SetField {
    Server,
    Token,
}

struct App {
    tab: Tab,
    settings: Settings,
    api: Api,

    /// Locas known to the building, and which one is selected.
    locas: Vec<String>,
    loca_sel: ListState,

    /// Invites in the selected loca.
    invites: Vec<Invite>,
    invite_sel: ListState,
    /// Why the selected loca's invites could not be read, shown in its own
    /// pane: one loca refusing is that loca's state, not the screen's.
    invites_note: Option<String>,

    // issue form
    field: Field,
    f_loca: String,
    f_name: String,

    // settings form
    set_field: SetField,
    s_server: String,
    s_token: String,

    /// The invite just minted — shown big, because it is handed over by eye.
    minted: Option<(String, String, String)>, // (loca, name, token)
    /// Pending "really revoke?" for the highlighted invite.
    confirm_revoke: Option<Invite>,
    /// Pending "really re-send?" for the highlighted invite.
    confirm_reissue: Option<Invite>,

    status: String,
    status_at: Instant,
    quit: bool,
}

impl App {
    fn new(settings: Settings) -> Self {
        let b = settings.current();
        let api = Api::new(b.server.clone(), b.admin_token.clone());
        let mut app = App {
            tab: Tab::Invites,
            s_server: b.server.clone(),
            s_token: b.admin_token.clone(),
            settings,
            api,
            locas: Vec::new(),
            loca_sel: ListState::default(),
            invites: Vec::new(),
            invite_sel: ListState::default(),
            invites_note: None,
            field: Field::Loca,
            f_loca: String::new(),
            f_name: String::new(),
            set_field: SetField::Server,
            minted: None,
            confirm_revoke: None,
            confirm_reissue: None,
            status: String::new(),
            status_at: Instant::now(),
            quit: false,
        };
        if app.settings.is_configured() {
            app.refresh();
        } else {
            app.tab = Tab::Settings;
            app.say("welcome — set the building address and the master key, then press s");
        }
        app
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_at = Instant::now();
    }

    /// Reload locas, then the selected loca's invites.
    fn refresh(&mut self) {
        match self.api.locas() {
            Ok(mut l) => {
                l.sort();
                let keep = self.selected_loca();
                self.locas = l;
                let idx = keep
                    .and_then(|k| self.locas.iter().position(|x| *x == k))
                    .unwrap_or(0);
                if !self.locas.is_empty() {
                    self.loca_sel.select(Some(idx.min(self.locas.len() - 1)));
                }
                self.load_invites();
            }
            Err(e) => self.say(format!("cannot reach the building: {e}")),
        }
    }

    fn selected_loca(&self) -> Option<String> {
        self.loca_sel
            .selected()
            .and_then(|i| self.locas.get(i))
            .cloned()
    }

    fn load_invites(&mut self) {
        self.invites_note = None;
        let Some(loca) = self.selected_loca() else {
            self.invites.clear();
            return;
        };
        match self.api.invites(&loca) {
            Ok(d) => {
                self.invites = d;
                self.invite_sel.select(if self.invites.is_empty() {
                    None
                } else {
                    Some(0)
                });
            }
            Err(e) => {
                // Never leave the previous loca's invites under this one's name.
                self.invites.clear();
                self.invite_sel.select(None);
                self.invites_note = Some(format!("could not read invites: {e}"));
            }
        }
    }

    fn issue(&mut self) {
        let loca = self.f_loca.trim().to_string();
        let name = self.f_name.trim().to_string();
        if loca.is_empty() || name.is_empty() {
            self.say("an invite needs a loca and a name");
            return;
        }
        match self.api.issue(&loca, &name) {
            Ok(token) => {
                self.minted = Some((loca.clone(), name.clone(), token));
                self.f_name.clear();
                self.refresh();
                self.say(format!("{name} is invited to {loca}"));
            }
            Err(e) => self.say(format!("could not issue: {e}")),
        }
    }

    fn revoke(&mut self, inv: &Invite) {
        match self.api.revoke(&inv.room, &inv.id) {
            Ok(()) => {
                self.say(format!("{}'s invite ended", inv.name));
                self.load_invites();
            }
            Err(e) => self.say(format!("could not revoke: {e}")),
        }
    }

    /// A fresh invite for the seat the holder already has; the old one ends.
    fn reissue(&mut self, inv: &Invite) {
        match self.api.reissue(&inv.room, &inv.name) {
            Ok(Reissued::Fresh(token)) => {
                self.minted = Some((inv.room.clone(), inv.name.clone(), token));
                self.load_invites();
                self.say(format!(
                    "{} has a fresh invite; the previous one no longer works",
                    inv.name
                ));
            }
            Ok(Reissued::Unsupported) => self.say(format!(
                "this building cannot re-send yet; {}'s invite was not changed",
                inv.name
            )),
            Err(e) => self.say(format!("could not re-send: {e}")),
        }
    }

    fn save_settings(&mut self) {
        let (server, token) = (
            self.s_server.trim().trim_end_matches('/').to_string(),
            self.s_token.trim().to_string(),
        );
        if let Some(b) = self.settings.current_mut() {
            b.server = server;
            b.admin_token = token;
        }
        match self.settings.save() {
            Ok(path) => self.saved(&path),
            Err(e) => self.say(format!("could not save: {e}")),
        }
    }

    /// The settings are on disk: say so, then try them. Said first on
    /// purpose: if the building refuses the key or cannot be reached, that is
    /// what the operator must read, and it must not be covered by "saved".
    fn saved(&mut self, path: &std::path::Path) {
        self.say(format!("saved to {} (600)", path.display()));
        self.reconnect();
    }

    /// Switch buildings: address and key move together, always. Sending prod's
    /// key to localhost is how you lock yourself out.
    fn switch_building(&mut self) {
        self.settings.next_building();
        let b = self.settings.current();
        self.s_server = b.server.clone();
        self.s_token = b.admin_token.clone();
        self.reconnect();
        self.say(format!("now on {} ({})", b.label, b.server));
    }

    fn reconnect(&mut self) {
        let b = self.settings.current();
        self.api = Api::new(b.server.clone(), b.admin_token.clone());
        self.invites.clear();
        self.invites_note = None;
        self.locas.clear();
        if self.settings.is_configured() {
            self.refresh();
        } else {
            self.tab = Tab::Settings;
            self.say(format!("{} has no master key yet", b.label));
        }
    }
}

fn main() -> io::Result<()> {
    let settings = Settings::load();
    let mut term = enter()?;
    let mut app = App::new(settings);

    while !app.quit {
        term.draw(|f| ui(f, &mut app))?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    on_key(&mut app, k.code, k.modifiers);
                }
            }
        }
        if app.status_at.elapsed() > Duration::from_secs(6) && !app.status.is_empty() {
            app.status.clear();
        }
    }

    leave(term)
}

fn on_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    // A minted invite is meant to be read and copied; any key dismisses it.
    if app.minted.is_some() {
        if code == KeyCode::Char('c') {
            let token = app.minted.as_ref().unwrap().2.clone();
            if copy_to_clipboard(&token) {
                app.say("copied — hand it over privately, never into a room");
            } else {
                app.say("no clipboard here; select the token with the mouse");
            }
            return;
        }
        app.minted = None;
        return;
    }
    if let Some(inv) = app.confirm_revoke.clone() {
        match code {
            KeyCode::Char('y') => {
                app.confirm_revoke = None;
                app.revoke(&inv);
            }
            _ => {
                app.confirm_revoke = None;
                app.say("kept");
            }
        }
        return;
    }

    if let Some(inv) = app.confirm_reissue.clone() {
        app.confirm_reissue = None;
        if code == KeyCode::Char('y') {
            app.reissue(&inv);
        } else {
            app.say("kept");
        }
        return;
    }
    if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }

    // Typing screens keep their letters; only Tab/Esc/F-keys navigate.
    let typing = matches!(app.tab, Tab::Issue | Tab::Settings);

    match code {
        KeyCode::Tab => {
            let all = Tab::all();
            let i = all.iter().position(|t| *t == app.tab).unwrap_or(0);
            app.tab = all[(i + 1) % all.len()];
        }
        KeyCode::Esc => app.quit = true,
        KeyCode::Char('q') if !typing => app.quit = true,
        KeyCode::Char('r') if !typing => {
            app.refresh();
            app.say("refreshed");
        }
        KeyCode::F(2) => app.switch_building(),
        KeyCode::Char('b') if !typing => app.switch_building(),
        _ => match app.tab {
            Tab::Invites => invites_key(app, code),
            Tab::Issue => issue_key(app, code),
            Tab::Settings => settings_key(app, code),
        },
    }
}

fn invites_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            let i = app.invite_sel.selected().unwrap_or(0);
            app.invite_sel.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let i = app.invite_sel.selected().unwrap_or(0);
            if i + 1 < app.invites.len() {
                app.invite_sel.select(Some(i + 1));
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            let i = app.loca_sel.selected().unwrap_or(0);
            app.loca_sel.select(Some(i.saturating_sub(1)));
            app.load_invites();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            let i = app.loca_sel.selected().unwrap_or(0);
            if i + 1 < app.locas.len() {
                app.loca_sel.select(Some(i + 1));
                app.load_invites();
            }
        }
        KeyCode::Char('n') => {
            app.f_loca = app.selected_loca().unwrap_or_default();
            app.tab = Tab::Issue;
            app.field = Field::Name;
        }
        KeyCode::Char('s') => {
            if let Some(i) = app.invite_sel.selected() {
                if let Some(inv) = app.invites.get(i) {
                    app.confirm_reissue = Some(inv.clone());
                }
            }
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if let Some(i) = app.invite_sel.selected() {
                if let Some(inv) = app.invites.get(i) {
                    app.confirm_revoke = Some(inv.clone());
                }
            }
        }
        _ => {}
    }
}

fn issue_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up => app.field = Field::Loca,
        KeyCode::Down => app.field = Field::Name,
        KeyCode::Enter => app.issue(),
        KeyCode::Backspace => {
            match app.field {
                Field::Loca => app.f_loca.pop(),
                Field::Name => app.f_name.pop(),
            };
        }
        KeyCode::Char(c) => match app.field {
            Field::Loca => app.f_loca.push(c),
            Field::Name => app.f_name.push(c),
        },
        _ => {}
    }
}

fn settings_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up => app.set_field = SetField::Server,
        KeyCode::Down => app.set_field = SetField::Token,
        KeyCode::Enter => app.save_settings(),
        KeyCode::Backspace => {
            match app.set_field {
                SetField::Server => app.s_server.pop(),
                SetField::Token => app.s_token.pop(),
            };
        }
        KeyCode::Char(c) => match app.set_field {
            SetField::Server => app.s_server.push(c),
            SetField::Token => app.s_token.push(c),
        },
        _ => {}
    }
}

// ---- drawing ----

fn ui(f: &mut Frame, app: &mut App) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(f.area());

    let titles: Vec<Line> = Tab::all().iter().map(|t| Line::from(t.title())).collect();
    let sel = Tab::all().iter().position(|t| *t == app.tab).unwrap_or(0);
    f.render_widget(
        Tabs::new(titles)
            .select(sel)
            .highlight_style(Style::new().fg(Color::Black).bg(Color::Cyan))
            .block(Block::default().borders(Borders::ALL).title(format!(
                " loca — master console · {} ",
                app.settings.current().label
            ))),
        rows[0],
    );

    match app.tab {
        Tab::Invites => draw_invites(f, app, rows[1]),
        Tab::Issue => draw_issue(f, app, rows[1]),
        Tab::Settings => draw_settings(f, app, rows[1]),
    }

    let help = match app.tab {
        Tab::Invites => {
            "←→ loca · ↑↓ invite · n new · s re-send · d revoke · b building · r refresh · q"
        }
        Tab::Issue => "↑↓ field · enter issue · tab · esc",
        Tab::Settings => "↑↓ field · enter save · F2 building · tab · esc",
    };
    let foot = if app.status.is_empty() {
        Line::from(help).style(Style::new().fg(Color::DarkGray))
    } else {
        Line::from(app.status.as_str()).style(Style::new().fg(Color::Yellow))
    };
    f.render_widget(
        Paragraph::new(foot).block(Block::default().borders(Borders::ALL)),
        rows[2],
    );

    if let Some((loca, name, token)) = app.minted.clone() {
        draw_minted(f, &loca, &name, &token);
    } else if let Some(inv) = app.confirm_revoke.clone() {
        draw_confirm(f, &inv);
    } else if let Some(inv) = app.confirm_reissue.clone() {
        draw_confirm_reissue(f, &inv);
    }
}

fn draw_invites(f: &mut Frame, app: &mut App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Length(22), Constraint::Min(20)]).split(area);

    let locas: Vec<ListItem> = app
        .locas
        .iter()
        .map(|l| ListItem::new(l.as_str()))
        .collect();
    f.render_stateful_widget(
        List::new(locas)
            .block(Block::default().borders(Borders::ALL).title(" locas "))
            .highlight_style(Style::new().fg(Color::Black).bg(Color::Cyan)),
        cols[0],
        &mut app.loca_sel,
    );

    let title = match app.selected_loca() {
        Some(l) if app.invites_note.is_some() => format!(" invites in {l} "),
        Some(l) => format!(" invites in {l} ({} seats taken) ", app.invites.len()),
        None => " invites ".into(),
    };
    let items: Vec<ListItem> = app
        .invites
        .iter()
        .map(|d| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<16}", d.name), Style::new().fg(Color::White)),
                Span::styled(format!("{:<7}", d.kind), Style::new().fg(Color::DarkGray)),
                Span::styled(short(&d.id), Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = if let Some(note) = &app.invites_note {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            note.clone(),
            Style::new().fg(Color::Yellow),
        )))])
    } else if items.is_empty() {
        List::new(vec![ListItem::new(Line::from(Span::styled(
            "nobody is invited here yet — press n",
            Style::new().fg(Color::DarkGray),
        )))])
    } else {
        List::new(items)
    };
    f.render_stateful_widget(
        list.block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::new().fg(Color::Black).bg(Color::Cyan)),
        cols[1],
        &mut app.invite_sel,
    );
}

fn draw_issue(f: &mut Frame, app: &App, area: Rect) {
    let b = Block::default()
        .borders(Borders::ALL)
        .title(" issue an invite ");
    let inner = b.inner(area);
    f.render_widget(b, area);

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .split(inner);

    f.render_widget(
        field_line("loca ", &app.f_loca, app.field == Field::Loca),
        rows[0],
    );
    f.render_widget(
        field_line("name ", &app.f_name, app.field == Field::Name),
        rows[1],
    );
    // Agent or user is not asked here: an invite seats an existing member, and
    // the building takes the kind from that membership.
    f.render_widget(
        Paragraph::new(Span::styled(
            "  kind  the member's own (agent or user)",
            Style::new().fg(Color::DarkGray),
        )),
        rows[2],
    );

    f.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "An invite opens this loca and no other.",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "Hand it over privately — never into a room.",
                Style::new().fg(Color::DarkGray),
            )),
        ]))
        .wrap(Wrap { trim: true }),
        rows[3],
    );
}

fn draw_settings(f: &mut Frame, app: &App, area: Rect) {
    let b = Block::default().borders(Borders::ALL).title(" settings ");
    let inner = b.inner(area);
    f.render_widget(b, area);

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .split(inner);

    f.render_widget(
        field_line(
            &format!("{} ", app.settings.current().label),
            &app.s_server,
            app.set_field == SetField::Server,
        ),
        rows[0],
    );
    let masked = "•".repeat(app.s_token.chars().count().min(40));
    f.render_widget(
        field_line("master key ", &masked, app.set_field == SetField::Token),
        rows[1],
    );

    f.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "The master key is the building's own (its .env). It mints invites",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "and must never travel further than this machine.",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("stored at {} (600)", Settings::path().display()),
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "F2 switches building ({}). Each keeps its own key.",
                    app.settings
                        .buildings
                        .iter()
                        .map(|b| b.label.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                ),
                Style::new().fg(Color::DarkGray),
            )),
        ]))
        .wrap(Wrap { trim: true }),
        rows[2],
    );
}

fn field_line<'a>(label: &str, value: &str, active: bool) -> Paragraph<'a> {
    let mark = if active { "› " } else { "  " };
    let style = if active {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::Gray)
    };
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{mark}{label}"), style),
        Span::styled(
            value.to_string(),
            if active {
                Style::new()
                    .fg(Color::White)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::new().fg(Color::White)
            },
        ),
        Span::styled(if active { "_" } else { "" }, Style::new().fg(Color::Cyan)),
    ]))
}

/// The minted invite, big and alone — it exists to be read off the screen.
fn draw_minted(f: &mut Frame, loca: &str, name: &str, token: &str) {
    let area = centered(74, 11, f.area());
    f.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(name.to_string(), Style::new().fg(Color::White).bold()),
            Span::raw(" is invited to "),
            Span::styled(loca.to_string(), Style::new().fg(Color::Cyan).bold()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            token.to_string(),
            Style::new().fg(Color::Green),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Hand this over privately. Never paste it into a room.",
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "c copy · any other key close",
            Style::new().fg(Color::DarkGray),
        )),
    ]);
    f.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(Color::Green))
                    .title(" invite "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_confirm(f: &mut Frame, inv: &Invite) {
    let area = centered(60, 7, f.area());
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("End "),
                Span::styled(inv.name.clone(), Style::new().fg(Color::White).bold()),
                Span::raw("'s invite for "),
                Span::styled(inv.room.clone(), Style::new().fg(Color::Cyan)),
                Span::raw("?"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "y revoke · any other key keep",
                Style::new().fg(Color::DarkGray),
            )),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::Red))
                .title(" revoke "),
        ),
        area,
    );
}

fn draw_confirm_reissue(f: &mut Frame, inv: &Invite) {
    let area = centered(64, 8, f.area());
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("Re-send "),
                Span::styled(inv.name.clone(), Style::new().fg(Color::White).bold()),
                Span::raw("'s invite for "),
                Span::styled(inv.room.clone(), Style::new().fg(Color::Cyan)),
                Span::raw("?"),
            ]),
            Line::from("The current one stops working; the seat stays theirs."),
            Line::from(""),
            Line::from(Span::styled(
                "y re-send · any other key keep",
                Style::new().fg(Color::DarkGray),
            )),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::Yellow))
                .title(" re-send "),
        ),
        area,
    );
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

fn short(token: &str) -> String {
    token.chars().take(14).collect::<String>() + "…"
}

/// Best-effort clipboard via whatever the desktop provides. Absence is normal
/// (a bare server has none), so failure is reported, never fatal.
fn copy_to_clipboard(text: &str) -> bool {
    for (cmd, args) in [
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        ("pbcopy", &[][..]),
        ("clip.exe", &[][..]),
    ] {
        let child = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn();
        if let Ok(mut c) = child {
            if let Some(mut sin) = c.stdin.take() {
                let _ = sin.write_all(text.as_bytes());
            }
            if c.wait().map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn enter() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, terminal::EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(out))
}

fn leave(mut term: Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(term.backend_mut(), terminal::LeaveAlternateScreen)?;
    term.show_cursor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::io::{BufRead, BufReader};

    /// A building that answers fixed bodies by path, for as long as the test
    /// runs. Enough HTTP for `ureq`: one request per connection.
    fn building(routes: &'static [(&'static str, u16, &'static str)]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap_or("");
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, *b))
                    .unwrap_or((404, "no such loca"));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    fn console(server: String) -> App {
        App::new(Settings {
            buildings: vec![settings::Building {
                label: "scratch".into(),
                server,
                admin_token: "synthetic-master-key".into(),
            }],
            current: 0,
        })
    }

    fn screen(app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| ui(f, app)).unwrap();
        let buffer = term.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    /// A direct message is conversation only: the console does not offer it as a
    /// loca to manage invites for, so it is never selected first and refused.
    #[test]
    fn a_dm_is_not_listed_as_a_loca() {
        static ROUTES: &[(&str, u16, &str)] = &[
            (
                "/rooms",
                200,
                r#"[{"room":"bb-5aa44a1ddc8f0e107ce8bbdcd6cc327f"},{"room":"iye"},{"room":"zeta"}]"#,
            ),
            (
                "/rooms/bb-5aa44a1ddc8f0e107ce8bbdcd6cc327f/invites",
                403,
                "a direct message is conversation only",
            ),
            ("/rooms/iye/invites", 200, "[]"),
            ("/rooms/zeta/invites", 200, "[]"),
        ];
        let mut app = console(building(ROUTES));
        assert_eq!(app.locas, ["iye", "zeta"]);
        assert_eq!(app.selected_loca().as_deref(), Some("iye"));
        assert_eq!(app.status, "", "nothing failed");
        assert!(!screen(&mut app).contains("bb-"));
    }

    /// The form asks for a loca and a name only. An invite seats an existing
    /// member and the building takes agent or user from that membership, so
    /// the form offers no choice it cannot make, and every key typed in the
    /// name field is part of the name.
    #[test]
    fn the_invite_form_does_not_offer_a_kind_it_cannot_set() {
        static ROUTES: &[(&str, u16, &str)] = &[
            ("/rooms", 200, r#"[{"room":"alpha"}]"#),
            ("/rooms/alpha/invites", 200, "[]"),
        ];
        let mut app = console(building(ROUTES));
        app.tab = Tab::Issue;
        app.field = Field::Name;
        for key in ['u', 'm', 'a'] {
            issue_key(&mut app, KeyCode::Char(key));
        }
        assert_eq!(app.f_name, "uma");
        let shown = screen(&mut app);
        assert!(!shown.contains("to change"), "{shown}");
        assert!(shown.contains("the member's own"), "{shown}");
    }

    /// Saving a key the building refuses must not read as success: the
    /// refusal is the last thing said, and "saved" stays only when the
    /// building answers.
    #[test]
    fn saving_a_refused_key_says_it_was_refused() {
        static REFUSING: &[(&str, u16, &str)] = &[("/rooms", 401, "admin token required")];
        let mut app = console(building(REFUSING));
        app.saved(std::path::Path::new("/nowhere/admin.toml"));
        assert!(
            app.status.contains("refused") && !app.status.contains("saved"),
            "the operator reads why the building is empty: {:?}",
            app.status
        );

        static ANSWERING: &[(&str, u16, &str)] = &[
            ("/rooms", 200, r#"[{"room":"alpha"}]"#),
            ("/rooms/alpha/invites", 200, "[]"),
        ];
        let mut app = console(building(ANSWERING));
        app.saved(std::path::Path::new("/nowhere/admin.toml"));
        assert_eq!(app.status, "saved to /nowhere/admin.toml (600)");
    }

    /// One loca refusing its invites is that loca's state, shown in its pane;
    /// the rest of the screen, and the other locas, keep working.
    #[test]
    fn a_refused_loca_shows_it_in_its_own_pane() {
        static ROUTES: &[(&str, u16, &str)] = &[
            ("/rooms", 200, r#"[{"room":"alpha"},{"room":"beta"}]"#),
            (
                "/rooms/alpha/invites",
                200,
                r#"[{"id":"ivid_alpha_one","room":"alpha","name":"ada","kind":"agent"}]"#,
            ),
            ("/rooms/beta/invites", 403, "refused"),
        ];
        let mut app = console(building(ROUTES));
        assert_eq!(app.invites.len(), 1, "alpha's invites are read");
        invites_key(&mut app, KeyCode::Right);
        assert_eq!(app.selected_loca().as_deref(), Some("beta"));
        assert!(
            app.invites.is_empty(),
            "alpha's invites must not stay under beta's name"
        );
        assert_eq!(app.status, "", "the screen as a whole did not fail");
        let shown = screen(&mut app);
        let pane = shown
            .lines()
            .find(|line| line.contains("could not read invites"))
            .unwrap_or_else(|| panic!("beta's pane says why:\n{shown}"));
        assert!(pane.contains("the building answered 403"), "{pane}");
        let column = pane[..pane.find("could not read").unwrap()].chars().count();
        assert!(
            column > 22,
            "the note sits in the invites pane, right of the loca list: {pane}"
        );
        invites_key(&mut app, KeyCode::Left);
        assert_eq!(app.invites.len(), 1, "alpha still reads");
        assert!(!screen(&mut app).contains("could not read invites"));
    }
}
