//! The window: accounts and folders on the left, messages in the middle, the open message
//! (or the draft being written) on the right.
//!
//! All Graph work runs on background threads through [`MailApp::run`]; results come back to
//! the UI thread. Changes are applied optimistically and rolled back when Graph refuses.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, WindowExt};
use gpui_kit::*;
use jm_core::events::{Event, EventReader};
use jm_core::graph::{ListQuery, Page};
use jm_core::html::InlineImage;
use jm_core::{AccountConfig, Attachment, Body, Config, FlagStatus, Folder, Mailbox, Message, ids};

use crate::actions::{self, *};
use crate::composer::{Composer, ComposerEvent, Notice};
use crate::mailview::MailView;
use crate::recipients::AddressBook;
use crate::setup::{Setup, SetupEvent};
use crate::{i18n, tr};

const PAGE: u32 = 50;
const LIST_REFRESH: Duration = Duration::from_secs(60);
const FOLDER_REFRESH: Duration = Duration::from_secs(300);
/// Folder counts follow what was done to mail after this long: once for a run of changes.
const FOLDERS_SOON: Duration = Duration::from_millis(1500);
/// People to suggest change slowly: whom you wrote to lately.
const CONTACTS_REFRESH: Duration = Duration::from_secs(1800);

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

    /// Whether `message` lies in the trash (by its own folder, so also in *Pinned* and search).
    pub fn in_trash(&self, message: &Message) -> bool {
        let trash = self.folder("deleteditems").map(|f| f.id.as_str());
        trash.is_some() && message.parent_folder_id.as_deref() == trash
    }
}

/// Where [`MailApp::file_away`] puts a mail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FileTo {
    Archive,
    Trash,
    /// Out of the trash, back to the inbox.
    Inbox,
}

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    /// `folder` is a folder id or a well-known name.
    Folder { account: usize, folder: String },
    /// Pinned mail of all accounts (the Outlook follow-up flag; Spark calls it pinning).
    Pinned,
    Search { account: usize, query: String },
}

/// What the right-click menu of a row can do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowAction {
    Reply,
    ReplyAll,
    Forward,
    Pin,
    Read,
    Archive,
    Trash,
    MoveToInbox,
    CopyId,
}

pub struct Row {
    pub account: usize,
    pub message: Message,
}

pub struct Opened {
    pub account: usize,
    pub message: Message,
    pub attachments: Vec<Attachment>,
    /// The pictures the body embeds (logos in signatures, pasted screenshots), once fetched.
    pub pictures: Vec<InlineImage>,
    pub has_remote_images: bool,
    pub images_loaded: bool,
    /// What came after it in the conversation (answers, ours and drafts included), oldest first.
    pub thread: Vec<ThreadMessage>,
}

/// A body as HTML; plain text becomes HTML.
pub(crate) fn body_html(body: Option<&Body>) -> String {
    match body {
        Some(b) if b.is_html() => b.content.clone(),
        Some(b) => jm_core::html::text_to_html(&b.content),
        None => String::new(),
    }
}

/// A later message of the open mail's conversation, shown below it with only its new part.
pub struct ThreadMessage {
    pub message: Message,
    pub pictures: Vec<InlineImage>,
}

impl ThreadMessage {
    /// What a message adds, as HTML: Graph's `unique_body`, else its preview.
    pub(crate) fn new_part(message: &Message) -> String {
        match message.unique_body.as_ref().filter(|b| !b.content.trim().is_empty()) {
            Some(b) => body_html(Some(b)),
            None => jm_core::html::text_to_html(&message.body_preview),
        }
    }
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
    /// Row a context menu action applies to (instead of the selection).
    menu_target: Option<String>,
    /// Mail on its way to another folder: a list that was fetched before the move must not show
    /// it again.
    filing: HashSet<String>,
    /// A reply or forward is being made (its number, counted in `answers`): the editor opens
    /// when it is there, unless something else was opened meanwhile.
    answering: Option<u64>,
    answers: u64,
    /// Has the keyboard while a reply is being made: what is typed ahead must not be the
    /// list's shortcuts.
    wait_focus: FocusHandle,
    /// Accounts whose folder counts are to be fetched soon (see `FOLDERS_SOON`).
    folders_due: HashSet<usize>,
    /// The open mail's body and conversation (WebKit); `None` if it could not be made.
    pub mail_view: Option<MailView>,
    last_folder_refresh: Instant,
    last_contacts_refresh: Instant,
    /// The unread count on the Dock icon, as last set.
    badge: u32,
    /// Listens to the composer or the settings in the pane.
    pane_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl MailApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("Search", "Suchen"))
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
        let mail_view = match MailView::new(window) {
            Ok((view, mut messages)) => {
                cx.spawn_in(window, async move |this, cx| {
                    while let Some(message) = messages.next().await {
                        if this.update_in(cx, |this, window, cx| this.on_web_message(message, window, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                Some(view)
            }
            Err(e) => {
                eprintln!("no web view for mail: {e}");
                None
            }
        };
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
            menu_target: None,
            filing: HashSet::new(),
            answering: None,
            answers: 0,
            wait_focus: cx.focus_handle(),
            folders_due: HashSet::new(),
            mail_view,
            last_folder_refresh: Instant::now(),
            last_contacts_refresh: Instant::now(),
            badge: 0,
            pane_subscription: None,
            _subscriptions: subscriptions,
        };
        app.reload_accounts(window, cx);
        app.start_background_loops(window, cx);
        if std::env::var("JUST_MAIL_START").as_deref() == Ok("setup") {
            app.open_setup(window, cx);
        }
        app
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.list_focus.focus(window, cx);
    }

    /// What the right column shows. A reply that is still being made no longer opens then.
    fn set_pane(&mut self, pane: Pane) {
        self.answering = None;
        if !matches!(pane, Pane::Composer(_) | Pane::Setup(_)) {
            self.pane_subscription = None;
        }
        self.pane = pane;
    }

    /// Take the keyboard back from the mail's web view, if it has it.
    fn release_keyboard(&self, cx: &App) {
        if let Some(view) = &self.mail_view {
            view.release_keyboard(cx);
        }
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
        // an account gone or made read-only no longer counts
        self.update_badge();
        if self.accounts.is_empty() {
            self.view = None;
            self.rows.clear();
            self.open_setup(window, cx);
            return;
        }
        for ix in 0..self.accounts.len() {
            self.load_folders(ix, window, cx);
            self.load_contacts(ix, window, cx);
        }
        let view_valid = match &self.view {
            Some(View::Folder { account, .. } | View::Search { account, .. }) => *account < self.accounts.len(),
            Some(View::Pinned) => true,
            None => false,
        };
        if !view_valid {
            self.show(View::Folder { account: 0, folder: "inbox".into() }, window, cx);
        }
        if matches!(self.pane, Pane::Setup(_)) && self.config.accounts.iter().all(|a| a.id.is_empty()) {
            self.set_pane(Pane::Empty);
        }
        cx.notify();
    }

    /// Everything fresh from disk and server (⌘R): settings and sign-ins, folders, the list,
    /// recipient suggestions and the open mail. An open draft comes again only when it is saved;
    /// with edits on their way it stays as it is.
    pub fn reload_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let account_id = |ix: usize| self.accounts.get(ix).map(|a| a.config.id.clone());
        let open = match &self.pane {
            Pane::Reader(opened) => account_id(opened.account).map(|a| (a, opened.message.id.clone(), false)),
            Pane::Composer(composer) => composer.read(cx).reloadable().map(|(a, id)| (a, id, true)),
            _ => None,
        };
        self.reload_accounts(window, cx);
        if self.accounts.is_empty() {
            return; // the setup shows
        }
        self.last_folder_refresh = Instant::now();
        self.last_contacts_refresh = Instant::now();
        self.load_list(false, window, cx);
        let Some((account, id, draft)) = open else { return };
        let Some(ix) = self.accounts.iter().position(|a| a.config.id == account) else {
            self.set_pane(Pane::Empty);
            return;
        };
        if draft {
            self.load_draft(ix, id, window, cx);
        } else {
            if let Some(view) = &mut self.mail_view {
                view.forget();
            }
            self.open_message(ix, id, window, cx);
        }
    }

    pub fn open_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let setup = cx.new(|cx| Setup::new(window, cx));
        let subscription = cx.subscribe_in(&setup, window, |this, _, ev: &SetupEvent, window, cx| match ev {
            SetupEvent::AccountsChanged => this.reload_accounts(window, cx),
            SetupEvent::Close => {
                this.set_pane(Pane::Empty);
                cx.notify();
            }
        });
        self.set_pane(Pane::Setup(setup));
        self.pane_subscription = Some(subscription);
        cx.notify();
    }

    /// The folder counts of account `ix` soon: once for all that was done to its mail meanwhile
    /// (walking through unread mail asks for them with every mail).
    fn load_folders_soon(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.folders_due.insert(ix) {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FOLDERS_SOON).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.folders_due.remove(&ix);
                this.load_folders(ix, window, cx);
            });
        })
        .detach();
    }

    fn load_folders(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.accounts.get(ix) else { return };
        let (mailbox, id) = (account.mailbox.clone(), account.config.id.clone());
        self.run(window, cx, move || mailbox.folders(), move |this, result, _, _| {
            let Some(account) = this.accounts.get_mut(ix).filter(|a| a.config.id == id) else { return };
            match result {
                Ok(folders) => {
                    account.folders = folders;
                    account.folders_error = None;
                }
                Err(e) => account.folders_error = Some(e.to_string()),
            }
            this.update_badge();
        });
    }

    /// Whom the account wrote to or heard from, for the recipient suggestions of its drafts.
    fn load_contacts(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.accounts.get(ix) else { return };
        if account.config.read_only {
            return;
        }
        let mailbox = account.mailbox.clone();
        let id = account.config.id.clone();
        self.run(window, cx, move || mailbox.contacts(), move |_, result, _, cx| match result {
            Ok(list) => AddressBook::set(id, list, cx),
            Err(e) => eprintln!("no recipient suggestions for {id}: {e}"),
        });
    }

    /// Count the inbox again (one request), so new mail shows in the sidebar and on the Dock
    /// icon within a minute, not only with the next round over all folders.
    fn count_inbox(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.accounts.get(ix) else { return };
        let (mailbox, id) = (account.mailbox.clone(), account.config.id.clone());
        self.run(window, cx, move || mailbox.folder("inbox"), move |this, result, _, _| {
            let (Some(account), Ok(counted)) = (this.accounts.get_mut(ix).filter(|a| a.config.id == id), result) else {
                return;
            };
            if let Some(inbox) = account.folders.iter_mut().find(|f| f.id == counted.id) {
                inbox.unread_item_count = counted.unread_item_count;
                inbox.total_item_count = counted.total_item_count;
                this.update_badge();
            }
        });
    }

    /// Unread mail in the inboxes on the Dock icon. Read-only accounts stay out: their mail is
    /// read by another system, and nothing here could mark it read.
    fn update_badge(&mut self) {
        let unread = self
            .accounts
            .iter()
            .filter(|a| !a.config.read_only)
            .filter_map(|a| a.folder("inbox"))
            .map(|f| f.unread_item_count)
            .sum();
        if unread != self.badge {
            self.badge = unread;
            crate::dock::set_badge(unread);
        }
    }

    // MARK: lists

    pub fn show(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if self.view.as_ref() != Some(&view) {
            self.rows.clear();
            self.selected = None;
            if !matches!(self.pane, Pane::Composer(_) | Pane::Setup(_)) {
                self.set_pane(Pane::Empty);
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
            View::Pinned => self.accounts.iter().enumerate().map(|(i, a)| (i, a.mailbox.clone())).collect(),
        };
        let work_view = view.clone();
        let pinned_first = self.in_folder("inbox");
        self.run(
            window,
            cx,
            move || {
                let mut out = Vec::new();
                for (ix, mailbox) in jobs {
                    let page = match &work_view {
                        View::Folder { folder, .. } => {
                            let page =
                                mailbox.list(&ListQuery { folder: Some(folder.clone()), top: PAGE, ..Default::default() });
                            // like Spark: pinned mail of the inbox on top, then everything else by date
                            match page {
                                Ok(mut page) if pinned_first => {
                                    let query = ListQuery { folder: Some(folder.clone()), top: 25, flagged: true, ..Default::default() };
                                    if let Ok(pinned) = mailbox.list(&query) {
                                        let ids: HashSet<&str> = pinned.messages.iter().map(|m| m.id.as_str()).collect();
                                        page.messages.retain(|m| !ids.contains(m.id.as_str()));
                                        page.messages.splice(0..0, pinned.messages);
                                    }
                                    Ok(page)
                                }
                                other => other,
                            }
                        }
                        View::Pinned => {
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
                if view == View::Pinned {
                    rows.sort_by_key(|r| std::cmp::Reverse(r.message.date()));
                }
                // what is on its way to another folder was still here when this was fetched
                rows.retain(|r| !this.filing.contains(&r.message.id));
                if silent {
                    // a refresh in the background keeps what is on the screen: all of it when it
                    // failed (the network is not back after a wake), and the pages below the first
                    if error.is_some() {
                        return;
                    }
                    let fresh: HashSet<String> = rows.iter().map(|r| r.message.id.clone()).collect();
                    let oldest = rows.last().and_then(|r| r.message.date());
                    let below = |r: &Row| !fresh.contains(&r.message.id) && r.message.date() < oldest;
                    if matches!(view, View::Folder { .. }) && this.rows.iter().any(below) {
                        rows.extend(std::mem::take(&mut this.rows).into_iter().filter(below));
                        next = std::mem::take(&mut this.next);
                    }
                }
                this.rows = rows;
                this.next = next;
                this.list_error = error.as_ref().map(|e| e.to_string());
                if let Some(e) = error {
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
                        (ix, link, page)
                    })
                    .collect::<Vec<_>>())
            },
            move |this, result, _, _| {
                this.loading_more = false;
                if this.generation != generation {
                    return;
                }
                for (ix, link, page) in result.unwrap_or_default() {
                    // not there this time: the list asks again when it is scrolled to its end
                    let Ok(page) = page else {
                        this.next.push((ix, link));
                        continue;
                    };
                    let known: HashSet<String> = this.rows.iter().map(|r| r.message.id.clone()).collect();
                    this.rows.extend(
                        page.messages.into_iter().filter(|m| !known.contains(&m.id)).map(|message| Row { account: ix, message }),
                    );
                    if let Some(link) = page.next_link {
                        this.next.push((ix, link));
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
        self.in_folder("drafts")
    }

    /// Whether the current view is the well-known folder `name` (`inbox`, `drafts`, …).
    pub fn in_folder(&self, name: &str) -> bool {
        match &self.view {
            Some(View::Folder { account, folder }) => {
                folder == name
                    || self.accounts.get(*account).and_then(|a| a.folder(folder)).and_then(|f| f.well_known.as_deref())
                        == Some(name)
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
                let (events, notices) = (reader.poll(), crate::composer::take_notices());
                let told = this.update_in(cx, |this, window, cx| {
                    this.on_notices(notices, window, cx);
                    if !events.is_empty() {
                        this.on_events(events, window, cx);
                    }
                });
                if told.is_err() {
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
                    let all_folders = this.last_folder_refresh.elapsed() > FOLDER_REFRESH;
                    if all_folders {
                        this.last_folder_refresh = Instant::now();
                    }
                    let contacts = this.last_contacts_refresh.elapsed() > CONTACTS_REFRESH;
                    if contacts {
                        this.last_contacts_refresh = Instant::now();
                    }
                    for ix in 0..this.accounts.len() {
                        if contacts {
                            this.load_contacts(ix, window, cx);
                        }
                        if all_folders {
                            this.load_folders(ix, window, cx);
                        } else {
                            this.count_inbox(ix, window, cx);
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

    /// What became of mail whose composer is gone, or was sent.
    fn on_notices(&mut self, notices: Vec<Notice>, window: &mut Window, cx: &mut Context<Self>) {
        for notice in notices {
            let note = match notice {
                Notice::Sent { recipients } => {
                    let text = if i18n::german() { format!("An {recipients}") } else { format!("To {recipients}") };
                    Notification::success(text).title(tr!("Sent", "Gesendet"))
                }
                // these stay until they are read
                Notice::NotSent(error) => Notification::error(error).title(tr!("Not sent", "Nicht gesendet")).autohide(false),
                Notice::NotSaved(error) => {
                    Notification::error(error).title(tr!("Draft not saved", "Entwurf nicht gespeichert")).autohide(false)
                }
            };
            window.push_notification(note, cx);
        }
    }

    fn on_events(&mut self, events: Vec<Event>, window: &mut Window, cx: &mut Context<Self>) {
        // several changes of one account: its folders and the list once
        let mut changed: Vec<usize> = Vec::new();
        for event in events {
            match event {
                Event::Changed { account, .. } => {
                    if let Some(ix) = self.accounts.iter().position(|a| a.config.id == account).filter(|ix| !changed.contains(ix)) {
                        changed.push(ix);
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
        let mut refresh = false;
        for ix in changed {
            refresh |= match &self.view {
                Some(View::Pinned) => true,
                Some(View::Folder { account, .. } | View::Search { account, .. }) => *account == ix,
                None => false,
            };
            self.load_folders(ix, window, cx);
            match &self.pane {
                // an answer drafted through jm shows up below the open mail
                Pane::Reader(opened) if opened.account == ix => self.load_thread(window, cx),
                // and a draft jm changed shows what it says now
                Pane::Composer(composer) if composer.read(cx).account() == ix => {
                    composer.clone().update(cx, |c, cx| c.refresh(window, cx));
                }
                _ => {}
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
        self.set_pane(Pane::Loading);
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
                    // its row is gone (unpinned in Pinned, say) and nothing else was opened
                    if this.selected.is_none() && matches!(this.pane, Pane::Loading) {
                        this.set_pane(Pane::Empty);
                    }
                    return;
                }
                let read_only = this.accounts.get(account).is_none_or(|a| a.config.read_only);
                match result {
                    Ok((message, attachments)) => {
                        // a draft opens in the editor, where it can be edited at all
                        if message.is_draft && !read_only {
                            this.open_draft(account, message.id.clone(), window, cx);
                            return;
                        }
                        let unread = !message.is_read;
                        let opened = reader_state(account, message, attachments, remote);
                        this.set_pane(Pane::Reader(Box::new(opened)));
                        if unread {
                            this.mark_read(true, window, cx);
                        }
                        this.load_pictures(window, cx);
                        this.load_thread(window, cx);
                    }
                    Err(e) => {
                        this.set_pane(Pane::Empty);
                        this.notify_error(tr!("Could not open the mail", "E-Mail konnte nicht geöffnet werden"), &e, window, cx);
                    }
                }
            },
        );
    }

    /// Fetch what came after the open mail in its conversation and show it below the mail. Mail
    /// in the trash or junk folder stays out; without an answer from Graph nothing is shown.
    fn load_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Reader(opened) = &self.pane else { return };
        let Some(conversation) = opened.message.conversation_id.clone() else { return };
        let Some(account) = self.accounts.get(opened.account) else { return };
        let mailbox = account.mailbox.clone();
        let hidden: Vec<String> = account
            .folders
            .iter()
            .filter(|f| matches!(f.well_known.as_deref(), Some("deleteditems" | "junkemail")))
            .map(|f| f.id.clone())
            .collect();
        let (id, after) = (opened.message.id.clone(), opened.message.date());
        let wanted = id.clone();
        self.run(
            window,
            cx,
            move || {
                let later = mailbox.conversation(&conversation)?.into_iter().filter(|m| {
                    m.id != id && m.date() > after && m.parent_folder_id.as_ref().is_none_or(|f| !hidden.contains(f))
                });
                Ok(later
                    .map(|m| {
                        let pictures = mailbox.inline_images(&m.id, &ThreadMessage::new_part(&m)).unwrap_or_default();
                        (m, pictures)
                    })
                    .collect::<Vec<_>>())
            },
            move |this, result, _, _| {
                let Pane::Reader(opened) = &mut this.pane else { return };
                let Ok(later) = result else { return };
                if opened.message.id != wanted {
                    return;
                }
                opened.thread = later.into_iter().map(|(message, pictures)| ThreadMessage { message, pictures }).collect();
            },
        );
    }

    /// Fetch the pictures the open mail embeds and show them; its text is there already.
    fn load_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Reader(opened) = &self.pane else { return };
        let html = body_html(opened.message.body.as_ref());
        if jm_core::html::content_ids(&html).is_empty() {
            return;
        }
        let Some(mailbox) = self.accounts.get(opened.account).map(|a| a.mailbox.clone()) else { return };
        let id = opened.message.id.clone();
        let wanted = id.clone();
        self.run(
            window,
            cx,
            move || mailbox.inline_images(&id, &html),
            move |this, result, _, _| {
                let (Pane::Reader(opened), Ok(pictures)) = (&mut this.pane, result) else { return };
                if opened.message.id == wanted && !pictures.is_empty() {
                    opened.pictures = pictures;
                }
            },
        );
    }

    /// Show a draft in the editor (it stays as it is if the editor shows it already).
    pub fn open_draft(&mut self, account: usize, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let open = matches!(&self.pane, Pane::Composer(c) if c.read(cx).account() == account && c.read(cx).draft_id() == Some(id.as_str()));
        if !open {
            self.load_draft(account, id, window, cx);
        }
    }

    /// Fetch a draft into a new editor.
    fn load_draft(&mut self, account: usize, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(acc) = self.accounts.get(account) else { return };
        if acc.config.read_only {
            // nothing of it could be saved: it shows like any mail
            self.open_message(account, id, window, cx);
            return;
        }
        let composer = cx.new(|cx| Composer::existing(account, acc.mailbox.clone(), self.mail_view.clone(), id, window, cx));
        self.set_composer(composer, window, cx);
    }

    fn set_composer(&mut self, composer: Entity<Composer>, window: &mut Window, cx: &mut Context<Self>) {
        let subscription = cx.subscribe_in(&composer, window, |this, _, ev: &ComposerEvent, window, cx| {
            this.on_composer_event(ev, window, cx)
        });
        self.set_pane(Pane::Composer(composer));
        self.pane_subscription = Some(subscription);
        cx.notify();
    }

    fn on_composer_event(&mut self, ev: &ComposerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ComposerEvent::Saved { account, message } => {
                // keep the drafts list in step with what was just saved
                if let Some(row) = self.rows.iter_mut().find(|r| r.message.id == message.id) {
                    row.message = (**message).clone();
                } else if self.in_drafts() && self.current_account() == Some(*account) {
                    self.rows.insert(0, Row { account: *account, message: (**message).clone() });
                }
                self.selected = Some(message.id.clone());
            }
            // that it was sent, and to whom, comes as a notice (see `on_notices`)
            ComposerEvent::Sent { id } => {
                self.drop_row(id);
                self.set_pane(Pane::Empty);
                self.focus_list(window, cx);
            }
            ComposerEvent::Discarded { account, id } => {
                if let Some(id) = id {
                    self.drop_row(id);
                }
                self.set_pane(Pane::Empty);
                self.load_folders_soon(*account, window, cx);
                self.focus_list(window, cx);
            }
            ComposerEvent::Failed { error } => {
                self.set_pane(Pane::Empty);
                self.notify_error(tr!("Could not open the draft", "Entwurf konnte nicht geöffnet werden"), error, window, cx);
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

    /// The message the actions apply to: the row of a context menu, else the open one, else the
    /// selected row.
    fn target(&self) -> Option<(usize, Message)> {
        if let Some(id) = &self.menu_target {
            return self.rows.iter().find(|r| &r.message.id == id).map(|r| (r.account, r.message.clone()));
        }
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
            } else if this.view == Some(View::Pinned) && after != FlagStatus::Flagged {
                this.drop_row(&id);
            } else if this.in_folder("inbox") {
                this.load_list(true, window, cx); // pinned mail moves to the top or back
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
                Ok(_) => this.load_folders_soon(account, window, cx),
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

    /// Archive or trash the target message, or take it out of the trash; the next one gets selected.
    pub fn file_away(&mut self, to: FileTo, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        if !self.writable(account, window, cx) {
            return;
        }
        let trashed = self.accounts[account].in_trash(&message);
        let pointless = match to {
            FileTo::Archive => message.is_draft,
            // moved there again it would only vanish from the list until the next load (drafts
            // are discarded instead)
            FileTo::Trash => trashed && !message.is_draft,
            FileTo::Inbox => !trashed || message.is_draft,
        };
        if pointless {
            return;
        }
        let id = message.id.clone();
        let ix = self.row_index(&id);
        // move on to the next mail only when the one filed away was the selected one
        let was_selected = self.selected.as_deref() == Some(id.as_str());
        let removed = self.drop_row(&id);
        self.filing.insert(id.clone());
        if was_selected && matches!(self.pane, Pane::Reader(_) | Pane::Composer(_) | Pane::Loading) {
            self.set_pane(Pane::Empty);
        }
        if let Some(ix) = ix.filter(|_| was_selected && !self.rows.is_empty()) {
            self.select_index(ix.min(self.rows.len() - 1), window, cx);
        }
        let mailbox = self.accounts[account].mailbox.clone();
        let is_draft = message.is_draft;
        let (work_id, view) = (id.clone(), self.view.clone());
        self.run(
            window,
            cx,
            move || {
                if is_draft {
                    return mailbox.delete_draft(&work_id);
                }
                match to {
                    FileTo::Archive => mailbox.archive(&work_id),
                    FileTo::Trash => mailbox.trash(&work_id),
                    FileTo::Inbox => mailbox.move_to(&work_id, "inbox"),
                }
                .map(|_| ())
            },
            move |this, result, window, cx| {
                this.filing.remove(&id);
                let Err(e) = result else {
                    this.load_folders_soon(account, window, cx);
                    return;
                };
                // back into the list it came from, if that is still the one on the screen
                if let (Some(row), Some(ix)) = (removed.filter(|_| this.view == view), ix) {
                    this.rows.insert(ix.min(this.rows.len()), row);
                }
                let title = match to {
                    FileTo::Archive => tr!("Could not archive", "Archivieren fehlgeschlagen"),
                    FileTo::Trash => tr!("Could not delete", "Löschen fehlgeschlagen"),
                    FileTo::Inbox => tr!("Could not move it to the inbox", "Nicht in den Posteingang verschoben"),
                };
                this.notify_error(title, &e, window, cx);
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
        let composer = cx.new(|cx| Composer::blank(account, acc.mailbox.clone(), self.mail_view.clone(), window, cx));
        self.selected = None;
        self.set_composer(composer, window, cx);
    }

    /// Reply (`all`), or forward when `forward` is set.
    pub fn answer(&mut self, all: bool, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        // one at a time: `r` typed twice is one reply
        if self.answering.is_some() || message.is_draft || !self.writable(account, window, cx) {
            return;
        }
        let mailbox = self.accounts[account].mailbox.clone();
        let id = message.id.clone();
        self.set_pane(Pane::Loading);
        self.answers += 1;
        let wanted = self.answers;
        self.answering = Some(wanted);
        // what is typed until the editor is there must not archive mail (`e`) or reply to all (`a`)
        self.wait_focus.focus(window, cx);
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
            move |this, result, window, cx| {
                // something else was opened meanwhile: the draft waits in Drafts
                if this.answering != Some(wanted) {
                    return;
                }
                match (result, this.accounts.get(account).map(|a| a.mailbox.clone())) {
                    (Ok(draft), Some(mailbox)) => {
                        let composer = cx.new(|cx| Composer::loaded(account, mailbox, this.mail_view.clone(), draft, window, cx));
                        this.selected = None;
                        this.set_composer(composer, window, cx);
                    }
                    (Ok(_), None) => this.set_pane(Pane::Empty),
                    (Err(e), _) => {
                        this.set_pane(Pane::Empty);
                        this.focus_list(window, cx);
                        this.notify_error(tr!("Could not create the draft", "Entwurf nicht angelegt"), &e, window, cx);
                    }
                }
            },
        );
        cx.notify();
    }

    /// Run a context menu action on the row with `id` (not on the selection).
    pub fn row_action(&mut self, id: String, action: RowAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_target = Some(id);
        match action {
            RowAction::Reply => self.answer(false, false, window, cx),
            RowAction::ReplyAll => self.answer(true, false, window, cx),
            RowAction::Forward => self.answer(false, true, window, cx),
            RowAction::Pin => self.toggle_flag(window, cx),
            RowAction::Read => self.toggle_read(window, cx),
            RowAction::Archive => self.file_away(FileTo::Archive, window, cx),
            RowAction::Trash => self.file_away(FileTo::Trash, window, cx),
            RowAction::MoveToInbox => self.file_away(FileTo::Inbox, window, cx),
            RowAction::CopyId => self.copy_id(window, cx),
        }
        self.menu_target = None;
    }

    pub fn copy_id(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, message)) = self.target() else { return };
        let Some(owner) = self.accounts.get(account) else { return };
        let short = ids::short(&message.id);
        ids::remember(&owner.config.id, [message.id.as_str()]);
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
            opened.images_loaded = true;
            cx.notify();
        }
    }

    pub fn download(&mut self, attachment: Attachment, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Reader(opened) = &self.pane else { return };
        let Some(mailbox) = self.accounts.get(opened.account).map(|a| a.mailbox.clone()) else { return };
        let message_id = opened.message.id.clone();
        self.run(
            window,
            cx,
            move || crate::util::save_attachment(&mailbox, &message_id, &attachment),
            |this, result, window, cx| match result {
                Ok(path) => cx.open_with_system(&path),
                Err(e) => this.notify_error(tr!("Download failed", "Download fehlgeschlagen"), &e, window, cx),
            },
        );
    }
}

/// Reader state for a fetched message; its pictures and conversation follow.
fn reader_state(account: usize, message: Message, attachments: Vec<Attachment>, remote: bool) -> Opened {
    let has_remote_images = jm_core::html::has_remote_images(&body_html(message.body.as_ref()));
    Opened {
        account,
        message,
        attachments,
        pictures: Vec::new(),
        has_remote_images,
        images_loaded: remote,
        thread: Vec::new(),
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
        // the web view lies on top of the window, so it has to go with the reader and the editor
        if let Some(view) = self.mail_view.as_ref().filter(|_| !matches!(self.pane, Pane::Reader(_) | Pane::Composer(_))) {
            view.hide(cx);
        }
        let pane: AnyElement = match &self.pane {
            Pane::Composer(c) => c.clone().into_any_element(),
            Pane::Setup(s) => s.clone().into_any_element(),
            _ => self.render_reader(window, cx).into_any_element(),
        };
        h_flex()
            .id("just-mail")
            .size_full()
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &NewMessage, w, cx| this.new_message(w, cx)))
            .on_action(cx.listener(|this, _: &actions::Reply, w, cx| this.answer(false, false, w, cx)))
            .on_action(cx.listener(|this, _: &ReplyAll, w, cx| this.answer(true, false, w, cx)))
            .on_action(cx.listener(|this, _: &Forward, w, cx| this.answer(false, true, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleFlag, w, cx| this.toggle_flag(w, cx)))
            .on_action(cx.listener(|this, _: &ToggleRead, w, cx| this.toggle_read(w, cx)))
            .on_action(cx.listener(|this, _: &ArchiveMessage, w, cx| this.file_away(FileTo::Archive, w, cx)))
            .on_action(cx.listener(|this, _: &TrashMessage, w, cx| this.file_away(FileTo::Trash, w, cx)))
            .on_action(cx.listener(|this, _: &CopyId, w, cx| this.copy_id(w, cx)))
            .on_action(cx.listener(|this, _: &actions::Refresh, w, cx| {
                this.load_list(true, w, cx);
                for ix in 0..this.accounts.len() {
                    this.load_folders(ix, w, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ReloadAll, w, cx| this.reload_all(w, cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, w, cx| {
                this.release_keyboard(cx); // ⌘F or `/` in the mail: the search gets the keys
                this.search.update(cx, |s, cx| s.focus(w, cx));
            }))
            // GPUI only sees clicks outside the mail: whatever was clicked gets the keys
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| this.release_keyboard(cx)))
            .on_action(cx.listener(|this, _: &SelectNext, w, cx| this.select_offset(1, w, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, w, cx| this.select_offset(-1, w, cx)))
            .child(sidebar)
            .child(list)
            .child(v_flex().flex_1().min_w_0().h_full().bg(theme.background).track_focus(&self.wait_focus).child(pane))
    }
}
