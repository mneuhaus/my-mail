//! The window: accounts and folders on the left, messages in the middle, the open message
//! (or the draft being written) on the right.
//!
//! All Graph work runs on background threads through [`MailApp::run`]; results come back to
//! the UI thread. Changes are applied optimistically and rolled back when Graph refuses.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, WindowExt};
use gpui_kit::*;
use jm_core::events::{Event, EventReader};
use jm_core::graph::{ListQuery, Page};
use jm_core::{AccountConfig, Attachment, Config, FlagStatus, Folder, Mailbox, Message, ids};

use crate::actions::{self, *};
use crate::composer::{Composer, ComposerEvent};
use crate::setup::{Setup, SetupEvent};
use crate::{i18n, tr};

const PAGE: u32 = 50;
const LIST_REFRESH: Duration = Duration::from_secs(60);
const FOLDER_REFRESH: Duration = Duration::from_secs(300);

pub struct Account {
    pub config: AccountConfig,
    pub mailbox: Arc<Mailbox>,
    pub folders: Vec<Folder>,
    pub folders_error: Option<String>,
    pub show_all_folders: bool,
}

impl Account {
    pub fn folder(&self, key: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == key || f.well_known.as_deref() == Some(key))
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    /// `folder` is a folder id or a well-known name.
    Folder { account: usize, folder: String },
    /// Flagged mail of all accounts.
    Flagged,
    Search { account: usize, query: String },
}

pub struct Row {
    pub account: usize,
    pub message: Message,
}

pub struct Opened {
    pub account: usize,
    pub message: Message,
    pub attachments: Vec<Attachment>,
    /// The body as Markdown (see `jm_core::html::to_display_markdown`).
    pub html: SharedString,
    pub has_remote_images: bool,
    pub images_loaded: bool,
}

pub enum Pane {
    Empty,
    Loading,
    Reader(Box<Opened>),
    Composer(Entity<Composer>),
    Setup(Entity<Setup>),
}

pub struct MailApp {
    pub config: Config,
    pub accounts: Vec<Account>,
    pub view: Option<View>,
    pub rows: Vec<Row>,
    /// Next-page links per account of the current view.
    pub next: Vec<(usize, String)>,
    pub loading: bool,
    pub loading_more: bool,
    pub list_error: Option<String>,
    pub selected: Option<String>,
    pub pane: Pane,
    pub search: Entity<InputState>,
    pub list_scroll: UniformListScrollHandle,
    pub list_focus: FocusHandle,
    /// Bumped on every list load so late answers for an old view are dropped.
    generation: u64,
    last_folder_refresh: Instant,
    _subscriptions: Vec<Subscription>,
}

impl MailApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("Search mail", "E-Mails durchsuchen"))
        });
        let subscriptions = vec![cx.subscribe_in(&search, window, |this, state, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::PressEnter { .. } => {
                    let query = state.read(cx).value().trim().to_string();
                    this.start_search(query, window, cx);
                }
                InputEvent::Change if state.read(cx).value().is_empty() => this.leave_search(window, cx),
                _ => {}
            }
        })];
        let mut app = Self {
            config: Config::default(),
            accounts: Vec::new(),
            view: None,
            rows: Vec::new(),
            next: Vec::new(),
            loading: false,
            loading_more: false,
            list_error: None,
            selected: None,
            pane: Pane::Empty,
            search,
            list_scroll: UniformListScrollHandle::new(),
            list_focus: cx.focus_handle(),
            generation: 0,
            last_folder_refresh: Instant::now(),
            _subscriptions: subscriptions,
        };
        app.reload_accounts(window, cx);
        app.start_background_loops(window, cx);
        app
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window, cx);
    }

    /// Run blocking core work on a background thread, then `done` on the UI thread.
    pub fn run<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> jm_core::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, jm_core::Result<T>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                done(this, result, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn notify_error(&self, title: &str, error: &jm_core::Error, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = error.message.clone();
        if let Some(hint) = &error.hint {
            text.push_str(&format!("\n{hint}"));
        }
        window.push_notification(Notification::error(text).title(title.to_string()), cx);
    }

    // MARK: accounts

    /// (Re)read config.toml and rebuild the account list, keeping loaded folders.
    pub fn reload_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                self.notify_error(tr!("Settings could not be read", "Einstellungen nicht lesbar"), &e, window, cx);
                Config::default()
            }
        };
        let old = std::mem::take(&mut self.accounts);
        for account in &self.config.accounts {
            let previous = old.iter().find(|a| a.config.id == account.id);
            self.accounts.push(Account {
                config: account.clone(),
                mailbox: Arc::new(Mailbox::new(account.clone(), self.config.client_id())),
                folders: previous.map(|p| p.folders.clone()).unwrap_or_default(),
                folders_error: None,
                show_all_folders: previous.is_some_and(|p| p.show_all_folders),
            });
        }
        if self.accounts.is_empty() {
            self.view = None;
            self.rows.clear();
            self.open_setup(window, cx);
            return;
        }
        for ix in 0..self.accounts.len() {
            self.load_folders(ix, window, cx);
        }
        let view_valid = match &self.view {
            Some(View::Folder { account, .. } | View::Search { account, .. }) => *account < self.accounts.len(),
            Some(View::Flagged) => true,
            None => false,
        };
        if !view_valid {
            self.show(View::Folder { account: 0, folder: "inbox".into() }, window, cx);
        }
        if matches!(self.pane, Pane::Setup(_)) && self.config.accounts.iter().all(|a| a.id.is_empty()) {
            self.pane = Pane::Empty;
        }
        cx.notify();
    }

    pub fn open_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let setup = cx.new(|cx| Setup::new(window, cx));
        self._subscriptions.push(cx.subscribe_in(&setup, window, |this, _, ev: &SetupEvent, window, cx| match ev {
            SetupEvent::AccountsChanged => this.reload_accounts(window, cx),
            SetupEvent::Close => {
                this.pane = Pane::Empty;
                cx.notify();
            }
        }));
        self.pane = Pane::Setup(setup);
        cx.notify();
    }

    fn load_folders(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mailbox = self.accounts[ix].mailbox.clone();
        let id = self.accounts[ix].config.id.clone();
        self.run(window, cx, move || mailbox.folders(), move |this, result, _, _| {
            let Some(account) = this.accounts.get_mut(ix).filter(|a| a.config.id == id) else { return };
            match result {
                Ok(folders) => {
                    account.folders = folders;
                    account.folders_error = None;
                }
                Err(e) => account.folders_error = Some(e.to_string()),
            }
        });
    }

    // MARK: lists

    pub fn show(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if self.view.as_ref() != Some(&view) {
            self.rows.clear();
            self.selected = None;
            if !matches!(self.pane, Pane::Composer(_) | Pane::Setup(_)) {
                self.pane = Pane::Empty;
            }
            self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.view = Some(view);
        self.load_list(false, window, cx);
    }

    /// Load the current view; `silent` keeps the current rows visible while it loads.
    pub fn load_list(&mut self, silent: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.view.clone() else { return };
        self.generation += 1;
        let generation = self.generation;
        if !silent {
            self.loading = true;
        }
        let jobs: Vec<(usize, Arc<Mailbox>)> = match &view {
            View::Folder { account, .. } | View::Search { account, .. } => {
                self.accounts.get(*account).map(|a| vec![(*account, a.mailbox.clone())]).unwrap_or_default()
            }
            View::Flagged => self.accounts.iter().enumerate().map(|(i, a)| (i, a.mailbox.clone())).collect(),
        };
        let work_view = view.clone();
        self.run(
            window,
            cx,
            move || {
                let mut out = Vec::new();
                for (ix, mailbox) in jobs {
                    let page = match &work_view {
                        View::Folder { folder, .. } => {
                            mailbox.list(&ListQuery { folder: Some(folder.clone()), top: PAGE, ..Default::default() })
                        }
                        View::Flagged => {
                            mailbox.list(&ListQuery { folder: None, top: 100, flagged: true, ..Default::default() })
                        }
                        View::Search { query, .. } => mailbox
                            .search(query, None, PAGE)
                            .map(|messages| Page { messages, next_link: None }),
                    };
                    if let Ok(page) = &page {
                        ids::remember(mailbox.id(), page.messages.iter().map(|m| m.id.as_str()));
                    }
                    out.push((ix, page));
                }
                Ok(out)
            },
            move |this, result: jm_core::Result<Vec<(usize, jm_core::Result<Page>)>>, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                let mut rows = Vec::new();
                let mut next = Vec::new();
                let mut error = None;
                for (ix, page) in result.unwrap_or_default() {
                    match page {
                        Ok(page) => {
                            rows.extend(page.messages.into_iter().map(|message| Row { account: ix, message }));
                            if let Some(link) = page.next_link {
                                next.push((ix, link));
                            }
                        }
                        Err(e) => error = Some(e),
                    }
                }
                if view == View::Flagged {
                    rows.sort_by(|a, b| b.message.date().cmp(&a.message.date()));
                }
                this.rows = rows;
                this.next = next;
                this.list_error = error.as_ref().map(|e| e.to_string());
                if let Some(e) = error.filter(|_| !silent) {
                    this.notify_error(tr!("Could not load mail", "E-Mails konnten nicht geladen werden"), &e, window, cx);
                }
            },
        );
    }

    /// Fetch the next page (when the list is scrolled to its end).
    pub fn load_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading_more || self.next.is_empty() {
            return;
        }
        self.loading_more = true;
        let generation = self.generation;
        let jobs: Vec<(usize, Arc<Mailbox>, String)> = std::mem::take(&mut self.next)
            .into_iter()
            .filter_map(|(ix, link)| self.accounts.get(ix).map(|a| (ix, a.mailbox.clone(), link)))
            .collect();
        self.run(
            window,
            cx,
            move || {
                Ok(jobs
                    .into_iter()
                    .map(|(ix, mailbox, link)| {
                        let page = mailbox.page(&link);
                        if let Ok(page) = &page {
                            ids::remember(mailbox.id(), page.messages.iter().map(|m| m.id.as_str()));
                        }
                        (ix, page)
                    })
                    .collect::<Vec<_>>())
            },
            move |this, result, _, _| {
                this.loading_more = false;
                if this.generation != generation {
                    return;
                }
                for (ix, page) in result.unwrap_or_default() {
                    if let Ok(page) = page {
                        let known: std::collections::HashSet<String> =
                            this.rows.iter().map(|r| r.message.id.clone()).collect();
                        this.rows.extend(
                            page.messages
                                .into_iter()
                                .filter(|m| !known.contains(&m.id))
                                .map(|message| Row { account: ix, message }),
                        );
                        if let Some(link) = page.next_link {
                            this.next.push((ix, link));
                        }
                    }
                }
            },
        );
    }

    fn start_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        if query.is_empty() {
            self.leave_search(window, cx);
            return;
        }
        let account = self.current_account().unwrap_or(0);
        self.show(View::Search { account, query }, window, cx);
    }

    fn leave_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(View::Search { account, .. }) = self.view.clone() {
            self.show(View::Folder { account, folder: "inbox".into() }, window, cx);
        }
    }

    /// The account the current view belongs to (`None` for the flagged view).
    pub fn current_account(&self) -> Option<usize> {
        match &self.view {
            Some(View::Folder { account, .. } | View::Search { account, .. }) => Some(*account),
            _ => None,
        }
    }

    /// Whether the current view lists drafts.
    pub fn in_drafts(&self) -> bool {
        match &self.view {
            Some(View::Folder { account, folder }) => {
                folder == "drafts"
                    || self.accounts.get(*account).and_then(|a| a.folder(folder)).and_then(|f| f.well_known.as_deref())
                        == Some("drafts")
            }
            _ => false,
        }
    }

    fn start_background_loops(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // CLI events: drafts created by `jm`, `jm open <id>`
        cx.spawn_in(window, async move |this, cx| {
            let mut reader = EventReader::new();
            loop {
                cx.background_executor().timer(Duration::from_millis(800)).await;
                let events = reader.poll();
                if !events.is_empty() && this.update_in(cx, |this, window, cx| this.on_events(events, window, cx)).is_err() {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
            }
        })
        .detach();
        // periodic refresh
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(LIST_REFRESH).await;
                let alive = this.update_in(cx, |this, window, cx| {
                    if !this.loading && !this.loading_more && !matches!(this.view, Some(View::Search { .. })) {
                        this.load_list(true, window, cx);
                    }
                    if this.last_folder_refresh.elapsed() > FOLDER_REFRESH {
                        this.last_folder_refresh = Instant::now();
                        for ix in 0..this.accounts.len() {
                            this.load_folders(ix, window, cx);
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_events(&mut self, events: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
        let mut refresh = false;
        for event in events {
            match event {
                Event::Changed { account, .. } => {
                    let ix = self.accounts.iter().position(|a| a.config.id == account);
                    let affects = match (&self.view, ix) {
                        (Some(View::Flagged), Some(_)) => true,
                        (Some(View::Folder { account, .. } | View::Search { account, .. }), Some(ix)) => *account == ix,
                        _ => false,
                    };
                    refresh |= affects;
                    if let Some(ix) = ix {
                        self.load_folders(ix, window, cx);
                    }
                }
                Event::Open { account, id } => {
                    if let Some(ix) = self.accounts.iter().position(|a| a.config.id == account) {
                        window.activate_window();
                        cx.activate(true);
                        self.open_message(ix, id, window, cx);
                    }
                }
            }
        }
        if refresh {
            self.load_list(true, window, cx);
        }
    }

    // MARK: selection

    fn row_index(&self, id: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.message.id == id)
    }

    pub fn select_index(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        let (account, id) = (row.account, row.message.id.clone());
        self.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        self.selected = Some(id.clone());
        if row.message.is_draft {
            self.open_draft(account, id, window, cx);
        } else {
            self.open_message(account, id, window, cx);
        }
    }

    fn select_offset(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.selected.as_deref().and_then(|id| self.row_index(id));
        let next = match current {
            Some(i) => (i as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize,
            None => 0,
        };
        if Some(next) != current {
            self.select_index(next, window, cx);
        }
    }

    /// Show a received message in the reader (and mark it read).
    pub fn open_message(&mut self, account: usize, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mailbox) = self.accounts.get(account).map(|a| a.mailbox.clone()) else { return };
        self.selected = Some(id.clone());
        self.pane = Pane::Loading;
        let remote = self.config.load_remote_images;
        let wanted = id.clone();
        self.run(
            window,
            cx,
            move || {
                let message = mailbox.message(&id)?;
                let attachments = if message.has_attachments {
                    mailbox.attachments(&id).unwrap_or_default().into_iter().filter(|a| !a.is_inline).collect()
                } else {
                    Vec::new()
                };
                ids::remember(mailbox.id(), [message.id.as_str()]);
                Ok((message, attachments))
            },
            move |this, result, window, cx| {
                if this.selected.as_deref() != Some(wanted.as_str()) {
                    return;
                }
                match result {
                    Ok((message, attachments)) => {
                        if message.is_draft {
                            this.open_draft(account, message.id.clone(), window, cx);
                            return;
                        }
                        let unread = !message.is_read;
                        let opened = reader_state(account, message, attachments, remote);
                        this.pane = Pane::Reader(Box::new(opened));
                        if unread {
                            this.mark_read(true, window, cx);
                        }
                    }
                    Err(e) => {
                        this.pane = Pane::Empty;
                        this.notify_error(tr!("Could not open the mail", "E-Mail konnte nicht geöffnet werden"), &e, window, cx);
                    }
                }
            },
        );
    }

    /// Show a draft in the editor.
    pub fn open_draft(&mut self, account: usize, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(acc) = self.accounts.get(account) else { return };
        let composer = cx.new(|cx| Composer::existing(account, acc.mailbox.clone(), id, window, cx));
        self.set_composer(composer, window, cx);
    }

    fn set_composer(&mut self, composer: Entity<Composer>, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.subscribe_in(&composer, window, |this, _, ev: &ComposerEvent, window, cx| {
            this.on_composer_event(ev, window, cx)
        }));
        self.pane = Pane::Composer(composer);
        cx.notify();
    }

    fn on_composer_event(&mut self, ev: &ComposerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ComposerEvent::Saved { account, message } => {
                // keep the drafts list in step with what was just saved
                if let Some(row) = self.rows.iter_mut().find(|r| r.message.id == message.id) {
                    row.message = message.clone();
                } else if self.in_drafts() && self.current_account() == Some(*account) {
                    self.rows.insert(0, Row { account: *account, message: message.clone() });
                }
                self.selected = Some(message.id.clone());
            }
            ComposerEvent::Sent { account, id, recipients } => {
                self.drop_row(id);
                self.pane = Pane::Empty;
                let text = if i18n::german() { format!("An {recipients}") } else { format!("To {recipients}") };
                window.push_notification(Notification::success(text).title(tr!("Sent", "Gesendet")), cx);
                self.load_folders(*account, window, cx);
                self.focus_list(window, cx);
            }
            ComposerEvent::Discarded { account, id } => {
                if let Some(id) = id {
                    self.drop_row(id);
                }
                self.pane = Pane::Empty;
                self.load_folders(*account, window, cx);
                self.focus_list(window, cx);
            }
        }
        cx.notify();
    }

    /// Remove a row and select its neighbour.
    fn drop_row(&mut self, id: &str) -> Option<Row> {
        let ix = self.row_index(id)?;
        let row = self.rows.remove(ix);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        }
        Some(row)
    }

    // MARK: message actions

    /// The message the actions apply to: the open one, else the selected row.
    fn target(&self) -> Option<(usize, Message)> {
        if let Pane::Reader(opened) = &self.pane {
            return Some((opened.account, opened.message.clone()));
        }
        let id = self.selected.as_deref()?;
        self.rows.iter().find(|r| r.message.id == id).map(|r| (r.account, r.message.clone()))
    }

    fn writable(&self, account: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let read_only = self.accounts.get(account).is_none_or(|a| a.config.read_only);
        if read_only {
            window.push_notification(
                Notification::warning(tr!(
                    "This account is read-only in Just Mail.",
                    "Dieses Konto ist in Just Mail nur lesbar."
                ))
                .title(tr!("Read-only", "Nur lesen")),
                cx,
            );
        }
        !read_only
    }

    /// Apply `edit` to the message in the list and in the reader.
    fn edit_message(&mut self, id: &str, edit: impl Fn(&mut Message)) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.message.id == id) {
            edit(&mut row.message);
        }
        if let Pane::Reader(opened) = &mut self.pane
            && opened.message.id == id
        {
            edit(&mut opened.message);
        }
    }

    pub fn toggle_flag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        if !self.writable(account, window, cx) {
            return;
        }
        let before = message.flag.flag_status;
        let after = if before == FlagStatus::Flagged { FlagStatus::NotFlagged } else { FlagStatus::Flagged };
        let id = message.id.clone();
        self.edit_message(&id, |m| m.flag.flag_status = after);
        let mailbox = self.accounts[account].mailbox.clone();
        let work_id = id.clone();
        self.run(window, cx, move || mailbox.set_flag(&work_id, after), move |this, result, window, cx| {
            if let Err(e) = result {
                this.edit_message(&id, |m| m.flag.flag_status = before);
                this.notify_error(tr!("Could not change the flag", "Markierung nicht geändert"), &e, window, cx);
            } else if this.view == Some(View::Flagged) && after != FlagStatus::Flagged {
                this.drop_row(&id);
            }
        });
        cx.notify();
    }

    fn mark_read(&mut self, read: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        if self.accounts.get(account).is_none_or(|a| a.config.read_only) {
            return; // read-only mailboxes keep their unread state (webIC relies on it)
        }
        let id = message.id.clone();
        self.edit_message(&id, |m| m.is_read = read);
        let mailbox = self.accounts[account].mailbox.clone();
        let work_id = id.clone();
        self.run(window, cx, move || mailbox.set_read(&work_id, read), move |this, result, window, cx| {
            match result {
                Ok(_) => this.load_folders(account, window, cx),
                Err(e) => {
                    this.edit_message(&id, |m| m.is_read = !read);
                    this.notify_error(tr!("Could not mark the mail", "E-Mail nicht markiert"), &e, window, cx);
                }
            }
        });
        cx.notify();
    }

    pub fn toggle_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        if self.writable(account, window, cx) {
            self.mark_read(!message.is_read, window, cx);
        }
    }

    /// Archive (`trash == false`) or trash the target message; the next one gets selected.
    pub fn file_away(&mut self, trash: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        if !self.writable(account, window, cx) {
            return;
        }
        if message.is_draft && !trash {
            return;
        }
        let id = message.id.clone();
        let ix = self.row_index(&id);
        let removed = self.drop_row(&id);
        if matches!(&self.pane, Pane::Reader(o) if o.message.id == id) || matches!(self.pane, Pane::Composer(_)) {
            self.pane = Pane::Empty;
        }
        if let Some(ix) = ix.filter(|_| !self.rows.is_empty()) {
            self.select_index(ix.min(self.rows.len() - 1), window, cx);
        }
        let mailbox = self.accounts[account].mailbox.clone();
        let is_draft = message.is_draft;
        self.run(
            window,
            cx,
            move || {
                if is_draft {
                    mailbox.delete_draft(&id)
                } else if trash {
                    mailbox.trash(&id).map(|_| ())
                } else {
                    mailbox.archive(&id).map(|_| ())
                }
            },
            move |this, result, window, cx| match result {
                Ok(()) => this.load_folders(account, window, cx),
                Err(e) => {
                    if let (Some(row), Some(ix)) = (removed, ix) {
                        this.rows.insert(ix.min(this.rows.len()), row);
                    }
                    let title = if trash { tr!("Could not delete", "Löschen fehlgeschlagen") } else { tr!("Could not archive", "Archivieren fehlgeschlagen") };
                    this.notify_error(title, &e, window, cx);
                }
            },
        );
        cx.notify();
    }

    pub fn new_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let account = self.current_account().unwrap_or(0);
        let Some(acc) = self.accounts.get(account) else { return };
        if acc.config.read_only {
            self.writable(account, window, cx);
            return;
        }
        let composer = cx.new(|cx| Composer::blank(account, acc.mailbox.clone(), window, cx));
        self.selected = None;
        self.set_composer(composer, window, cx);
    }

    /// Reply (`all`), or forward when `forward` is set.
    pub fn answer(&mut self, all: bool, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        if message.is_draft || !self.writable(account, window, cx) {
            return;
        }
        let mailbox = self.accounts[account].mailbox.clone();
        let id = message.id.clone();
        self.pane = Pane::Loading;
        self.run(
            window,
            cx,
            move || {
                if forward {
                    mailbox.create_forward(&id, &[], "", true)
                } else {
                    mailbox.create_reply(&id, all, "", true)
                }
            },
            move |this, result, window, cx| match result {
                Ok(draft) => {
                    let mailbox = this.accounts[account].mailbox.clone();
                    let composer = cx.new(|cx| Composer::loaded(account, mailbox, draft, window, cx));
                    this.selected = None;
                    this.set_composer(composer, window, cx);
                }
                Err(e) => {
                    this.pane = Pane::Empty;
                    this.notify_error(tr!("Could not create the draft", "Entwurf nicht angelegt"), &e, window, cx);
                }
            },
        );
        cx.notify();
    }

    pub fn copy_id(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        let short = ids::short(&message.id);
        ids::remember(&self.accounts[account].config.id, [message.id.as_str()]);
        cx.write_to_clipboard(ClipboardItem::new_string(short.clone()));
        let text = if i18n::german() {
            format!("{short} kopiert: `jm show {short}`")
        } else {
            format!("Copied {short}: `jm show {short}`")
        };
        window.push_notification(Notification::info(text), cx);
    }

    pub fn load_images(&mut self, cx: &mut Context<Self>) {
        if let Pane::Reader(opened) = &mut self.pane {
            let html = opened.message.body.as_ref().map(|b| b.content.clone()).unwrap_or_default();
            opened.html = jm_core::html::to_display_markdown(&html, true).into();
            opened.images_loaded = true;
            cx.notify();
        }
    }

    pub fn download(&mut self, attachment: Attachment, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Reader(opened) = &self.pane else { return };
        let mailbox = self.accounts[opened.account].mailbox.clone();
        let message_id = opened.message.id.clone();
        self.run(
            window,
            cx,
            move || {
                let bytes = mailbox.attachment_bytes(&message_id, &attachment.id)?;
                let path = crate::util::download_path(&attachment.name);
                std::fs::write(&path, bytes)?;
                Ok(path)
            },
            |this, result, window, cx| match result {
                Ok(path) => cx.open_with_system(&path),
                Err(e) => this.notify_error(tr!("Download failed", "Download fehlgeschlagen"), &e, window, cx),
            },
        );
    }
}

/// Reader state for a fetched message: sanitized HTML (plain text bodies become HTML too).
fn reader_state(account: usize, message: Message, attachments: Vec<Attachment>, remote: bool) -> Opened {
    let body = message.body.clone().unwrap_or_default();
    let raw = if body.is_html() { body.content } else { jm_core::html::text_to_html(&body.content) };
    let has_remote_images = jm_core::html::has_remote_images(&raw);
    Opened {
        account,
        html: jm_core::html::to_display_markdown(&raw, remote).into(),
        message,
        attachments,
        has_remote_images,
        images_loaded: remote,
    }
}

impl Focusable for MailApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.list_focus.clone()
    }
}

impl Render for MailApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sidebar = self.render_sidebar(window, cx);
        let list = self.render_list(window, cx);
        let pane: AnyElement = match &self.pane {
            Pane::Composer(c) => c.clone().into_any_element(),
            Pane::Setup(s) => s.clone().into_any_element(),
            _ => self.render_reader(window, cx).into_any_element(),
        };
        h_flex()
            .id("just-mail")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &NewMessage, w, cx| this.new_message(w, cx)))
            .on_action(cx.listener(|this, _: &actions::Reply, w, cx| this.answer(false, false, w, cx)))
            .on_action(cx.listener(|this, _: &ReplyAll, w, cx| this.answer(true, false, w, cx)))
            .on_action(cx.listener(|this, _: &Forward, w, cx| this.answer(false, true, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleFlag, w, cx| this.toggle_flag(w, cx)))
            .on_action(cx.listener(|this, _: &ToggleRead, w, cx| this.toggle_read(w, cx)))
            .on_action(cx.listener(|this, _: &ArchiveMessage, w, cx| this.file_away(false, w, cx)))
            .on_action(cx.listener(|this, _: &TrashMessage, w, cx| this.file_away(true, w, cx)))
            .on_action(cx.listener(|this, _: &CopyId, w, cx| this.copy_id(w, cx)))
            .on_action(cx.listener(|this, _: &actions::Refresh, w, cx| {
                this.load_list(true, w, cx);
                for ix in 0..this.accounts.len() {
                    this.load_folders(ix, w, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, w, cx| {
                this.search.update(cx, |s, cx| s.focus(w, cx));
            }))
            .on_action(cx.listener(|this, _: &SelectNext, w, cx| this.select_offset(1, w, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, w, cx| this.select_offset(-1, w, cx)))
            .child(sidebar)
            .child(list)
            .child(v_flex().flex_1().min_w_0().h_full().child(pane))
    }
}
