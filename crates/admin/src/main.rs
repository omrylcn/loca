//! loca-admin — the master's console: hand out davets, take them back.
//!
//! A davet is the right to enter one loca. Only the master issues one, and the
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

use api::{Api, Invite};
use settings::Settings;

/// Which screen is in front.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Davets,
    Issue,
    Settings,
}

impl Tab {
    fn all() -> [Tab; 3] {
        [Tab::Davets, Tab::Issue, Tab::Settings]
    }
    fn title(self) -> &'static str {
        match self {
            Tab::Davets => "davets",
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
    Kind,
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

    /// Davets in the selected loca.
    davets: Vec<Invite>,
    davet_sel: ListState,

    // issue form
    field: Field,
    f_loca: String,
    f_name: String,
    f_agent: bool,

    // settings form
    set_field: SetField,
    s_server: String,
    s_token: String,

    /// The davet just minted — shown big, because it is handed over by eye.
    minted: Option<(String, String, String)>, // (loca, name, token)
    /// Pending "really revoke?" for the highlighted davet.
    confirm_revoke: Option<Invite>,

    status: String,
    status_at: Instant,
    quit: bool,
}

impl App {
    fn new(settings: Settings) -> Self {
        let b = settings.current();
        let api = Api::new(b.server.clone(), b.admin_token.clone());
        let mut app = App {
            tab: Tab::Davets,
            s_server: b.server.clone(),
            s_token: b.admin_token.clone(),
            settings,
            api,
            locas: Vec::new(),
            loca_sel: ListState::default(),
            davets: Vec::new(),
            davet_sel: ListState::default(),
            field: Field::Loca,
            f_loca: String::new(),
            f_name: String::new(),
            f_agent: true,
            set_field: SetField::Server,
            minted: None,
            confirm_revoke: None,
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

    /// Reload locas, then the selected loca's davets.
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
                self.load_davets();
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

    fn load_davets(&mut self) {
        let Some(loca) = self.selected_loca() else {
            self.davets.clear();
            return;
        };
        match self.api.invites(&loca) {
            Ok(d) => {
                self.davets = d;
                self.davet_sel.select(if self.davets.is_empty() {
                    None
                } else {
                    Some(0)
                });
            }
            Err(e) => self.say(format!("could not read davets: {e}")),
        }
    }

    fn issue(&mut self) {
        let loca = self.f_loca.trim().to_string();
        let name = self.f_name.trim().to_string();
        if loca.is_empty() || name.is_empty() {
            self.say("a davet needs a loca and a name");
            return;
        }
        let kind = if self.f_agent { "agent" } else { "user" };
        match self.api.issue(&loca, &name, kind) {
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
                self.say(format!("{}'s davet ended", inv.name));
                self.load_davets();
            }
            Err(e) => self.say(format!("could not revoke: {e}")),
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
            Ok(path) => {
                self.reconnect();
                self.say(format!("saved to {} (600)", path.display()));
            }
            Err(e) => self.say(format!("could not save: {e}")),
        }
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
        self.davets.clear();
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
    // A minted davet is meant to be read and copied; any key dismisses it.
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
            Tab::Davets => davets_key(app, code),
            Tab::Issue => issue_key(app, code),
            Tab::Settings => settings_key(app, code),
        },
    }
}

fn davets_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            let i = app.davet_sel.selected().unwrap_or(0);
            app.davet_sel.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let i = app.davet_sel.selected().unwrap_or(0);
            if i + 1 < app.davets.len() {
                app.davet_sel.select(Some(i + 1));
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            let i = app.loca_sel.selected().unwrap_or(0);
            app.loca_sel.select(Some(i.saturating_sub(1)));
            app.load_davets();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            let i = app.loca_sel.selected().unwrap_or(0);
            if i + 1 < app.locas.len() {
                app.loca_sel.select(Some(i + 1));
                app.load_davets();
            }
        }
        KeyCode::Char('n') => {
            app.f_loca = app.selected_loca().unwrap_or_default();
            app.tab = Tab::Issue;
            app.field = Field::Name;
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if let Some(i) = app.davet_sel.selected() {
                if let Some(inv) = app.davets.get(i) {
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
        KeyCode::Char(' ') if app.field == Field::Kind => app.f_agent = !app.f_agent,
        KeyCode::Backspace => {
            match app.field {
                Field::Loca => app.f_loca.pop(),
                Field::Name => app.f_name.pop(),
                Field::Kind => None,
            };
        }
        KeyCode::Char(c) => match app.field {
            Field::Loca => app.f_loca.push(c),
            Field::Name => app.f_name.push(c),
            Field::Kind => {
                if c == 'a' {
                    app.f_agent = true
                } else if c == 'u' {
                    app.f_agent = false
                }
            }
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
        Tab::Davets => draw_davets(f, app, rows[1]),
        Tab::Issue => draw_issue(f, app, rows[1]),
        Tab::Settings => draw_settings(f, app, rows[1]),
    }

    let help = match app.tab {
        Tab::Davets => "←→ loca · ↑↓ davet · n new · d revoke · b building · r refresh · q",
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
    }
}

fn draw_davets(f: &mut Frame, app: &mut App, area: Rect) {
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
        Some(l) => format!(" davets in {l} ({}/7 seats) ", app.davets.len()),
        None => " davets ".into(),
    };
    let items: Vec<ListItem> = app
        .davets
        .iter()
        .map(|d| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<16}", d.name), Style::new().fg(Color::White)),
                Span::styled(format!("{:<7}", d.kind), Style::new().fg(Color::DarkGray)),
                Span::styled(short(&d.id), Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = if items.is_empty() {
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
        &mut app.davet_sel,
    );
}

fn draw_issue(f: &mut Frame, app: &App, area: Rect) {
    let b = Block::default()
        .borders(Borders::ALL)
        .title(" issue a davet ");
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
    let kind = if app.f_agent {
        "agent  (a/u to change)"
    } else {
        "user   (a/u to change)"
    };
    f.render_widget(field_line("kind ", kind, app.field == Field::Kind), rows[2]);

    f.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "A davet opens this loca and no other.",
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
                "The master key is the building's own (its .env). It mints davets",
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

/// The minted davet, big and alone — it exists to be read off the screen.
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
                    .title(" davet "),
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
                Span::raw("'s davet for "),
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
