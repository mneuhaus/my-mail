//! Accounts and signatures: sign in with a Microsoft device code, import ms365-mail sign-ins,
//! edit signatures, set accounts read-only.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jm_core::auth::{self, DeviceCode, Ms365Login};
use jm_core::Config;

use crate::sidebar::TOP_H;
use crate::{i18n, tr};

pub enum SetupEvent {
    AccountsChanged,
    Close,
}

impl EventEmitter<SetupEvent> for Setup {}

enum Login {
    Idle,
    Starting,
    Waiting(DeviceCode),
}

pub struct Setup {
    config: Config,
    signatures: Vec<(String, Entity<TextareaState>)>,
    imports: Vec<Ms365Login>,
    login: Login,
    busy: bool,
    message: Option<String>,
    error: Option<String>,
}

impl Setup {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            config: Config::default(),
            signatures: Vec::new(),
            imports: Vec::new(),
            login: Login::Idle,
            busy: false,
            message: None,
            error: None,
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config = Config::load().unwrap_or_default();
        self.signatures = self
            .config
            .accounts
            .iter()
            .map(|a| {
                let text = a.signature.clone();
                let state = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(4, 16)
                        .placeholder(tr!("Signature (plain text)", "Signatur (einfacher Text)"))
                        .default_value(text)
                });
                (a.id.clone(), state)
            })
            .collect();
        let known: Vec<String> = self.config.accounts.iter().map(|a| a.email.to_lowercase()).collect();
        self.imports = auth::find_ms365_logins(self.config.client_id())
            .into_iter()
            .filter(|l| !known.contains(&l.username.to_lowercase()))
            .collect();
        cx.notify();
    }

    fn run<T: Send + 'static>(
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

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client_id = self.config.client_id().to_string();
        self.login = Login::Starting;
        self.error = None;
        self.run(window, cx, move || auth::start_device_code(&client_id), |this, result, window, cx| match result {
            Ok(code) => {
                cx.write_to_clipboard(ClipboardItem::new_string(code.user_code.clone()));
                cx.open_url(&code.verification_uri);
                this.login = Login::Waiting(code.clone());
                let client_id = this.config.client_id().to_string();
                this.run(
                    window,
                    cx,
                    move || {
                        let token = auth::wait_device_code(&client_id, &code)?;
                        let mut config = Config::load()?;
                        auth::register_account(&mut config, token)
                    },
                    |this, result, window, cx| {
                        this.login = Login::Idle;
                        match result {
                            Ok(account) => {
                                this.message = Some(if i18n::german() {
                                    format!("{} hinzugefügt.", account.email)
                                } else {
                                    format!("Added {}.", account.email)
                                });
                                this.reload(window, cx);
                                cx.emit(SetupEvent::AccountsChanged);
                            }
                            Err(e) => this.error = Some(e.to_string()),
                        }
                    },
                );
            }
            Err(e) => {
                this.login = Login::Idle;
                this.error = Some(e.to_string());
            }
        });
    }

    fn import(&mut self, login: Ms365Login, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        self.run(
            window,
            cx,
            move || {
                let mut config = Config::load()?;
                auth::import_ms365_login(&mut config, &login)
            },
            |this, result, window, cx| {
                this.busy = false;
                match result {
                    Ok(account) => {
                        this.message = Some(if i18n::german() {
                            format!("{} übernommen.", account.email)
                        } else {
                            format!("Imported {}.", account.email)
                        });
                        this.reload(window, cx);
                        cx.emit(SetupEvent::AccountsChanged);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
            },
        );
    }

    /// Write signatures and read-only flags back to config.toml.
    fn save(&mut self, cx: &mut Context<Self>) {
        let mut config = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        for (id, state) in &self.signatures {
            let text = state.read(cx).value().trim_end().to_string();
            if let Some(a) = config.accounts.iter_mut().find(|a| &a.id == id) {
                a.signature = text;
            }
        }
        for a in &mut config.accounts {
            if let Some(local) = self.config.accounts.iter().find(|l| l.id == a.id) {
                a.read_only = local.read_only;
            }
        }
        match config.save() {
            Ok(()) => {
                self.config = config;
                self.message = Some(tr!("Saved.", "Gespeichert.").into());
                self.error = None;
                cx.emit(SetupEvent::AccountsChanged);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    fn remove(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(mut config) = Config::load() else { return };
        config.accounts.retain(|a| a.id != id);
        if let Err(e) = config.save() {
            self.error = Some(e.to_string());
            return;
        }
        let _ = std::fs::remove_dir_all(jm_core::paths::account_dir(&id));
        self.reload(window, cx);
        cx.emit(SetupEvent::AccountsChanged);
    }
}

impl Render for Setup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_accounts = !self.config.accounts.is_empty();
        let section = |title: &'static str| {
            div().mt_6().mb_2().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(title)
        };

        let mut accounts = v_flex().gap_4();
        for (ix, account) in self.config.accounts.iter().enumerate() {
            let signature = self.signatures.iter().find(|(id, _)| id == &account.id).map(|(_, s)| s.clone());
            let id = account.id.clone();
            let remove_id = account.id.clone();
            accounts = accounts.child(
                v_flex()
                    .gap_2()
                    .p_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(account.email.clone()))
                                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                                        "{}  ·  jm -a {}",
                                        account.name, account.id
                                    ))),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .text_xs()
                                    .child(Icon::new(IconName::Lock).xsmall())
                                    .child(tr!("read-only", "nur lesen"))
                                    .child(Switch::new(("ro", ix)).checked(account.read_only).on_click(cx.listener(
                                        move |this, checked: &bool, _, cx| {
                                            if let Some(a) = this.config.accounts.iter_mut().find(|a| a.id == id) {
                                                a.read_only = *checked;
                                            }
                                            this.save(cx);
                                        },
                                    ))),
                            )
                            .child(
                                Button::new(("remove", ix))
                                    .icon(IconName::Trash)
                                    .ghost()
                                    .xsmall()
                                    .tooltip(tr!("Remove account from Just Mail", "Konto aus Just Mail entfernen"))
                                    .on_click(cx.listener(move |this, _, w, cx| this.remove(remove_id.clone(), w, cx))),
                            ),
                    )
                    .when_some(signature, |d, s| {
                        d.child(div().text_xs().text_color(theme.muted_foreground).child(tr!("Signature", "Signatur")))
                            .child(Textarea::new(&s).small())
                    }),
            );
        }

        let mut add = v_flex().gap_3();
        match &self.login {
            Login::Waiting(code) => {
                add = add.child(
                    v_flex()
                        .gap_2()
                        .p_4()
                        .rounded(px(8.))
                        .bg(theme.muted)
                        .child(div().text_sm().child(tr!(
                            "Enter this code on the Microsoft page that just opened (it is on your clipboard):",
                            "Gib diesen Code auf der eben geöffneten Microsoft-Seite ein (er liegt in der Zwischenablage):"
                        )))
                        .child(div().text_2xl().font_weight(FontWeight::BOLD).child(code.user_code.clone()))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(code.verification_uri.clone())),
                );
            }
            _ => {
                add = add.child(
                    h_flex().gap_2().child(
                        Button::new("sign-in")
                            .primary()
                            .icon(IconName::LogIn)
                            .label(tr!("Sign in with Microsoft", "Mit Microsoft anmelden"))
                            .disabled(matches!(self.login, Login::Starting) || self.busy)
                            .on_click(cx.listener(|this, _, w, cx| this.sign_in(w, cx))),
                    ),
                );
            }
        }
        for (i, login) in self.imports.iter().enumerate() {
            let l = login.clone();
            add = add.child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_sm().flex_1().child(if i18n::german() {
                        format!("Anmeldung aus ms365-mail übernehmen: {} (Profil {})", login.username, login.profile)
                    } else {
                        format!("Use the ms365-mail sign-in of {} (profile {})", login.username, login.profile)
                    }))
                    .child(
                        Button::new(("import", i))
                            .label(tr!("Import", "Übernehmen"))
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, w, cx| this.import(l.clone(), w, cx))),
                    ),
            );
        }

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(TOP_H))
                    .flex_none()
                    .px_4()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .window_control_area(WindowControlArea::Drag)
                    .child(div().flex_1().text_base().font_weight(FontWeight::SEMIBOLD).child(tr!(
                        "Accounts & signatures",
                        "Konten & Signaturen"
                    )))
                    .when(has_accounts, |d| {
                        d.child(
                            Button::new("save-setup")
                                .primary()
                                .small()
                                .label(tr!("Save", "Speichern"))
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                        .child(
                            Button::new("close-setup")
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SetupEvent::Close))),
                        )
                    }),
            )
            .child(
                div().id("setup-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .max_w(px(720.))
                        .px_6()
                        .py_4()
                        .when(!has_accounts, |d| {
                            d.child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr!(
                                "Welcome to Just Mail",
                                "Willkommen bei Just Mail"
                            )))
                            .child(div().mt_1().text_sm().text_color(theme.muted_foreground).child(tr!(
                                "Sign in with your Microsoft 365 account to start.",
                                "Melde dich mit deinem Microsoft-365-Konto an, um loszulegen."
                            )))
                        })
                        .when_some(self.message.clone(), |d, m| {
                            d.child(div().mt_3().text_sm().text_color(theme.success).child(m))
                        })
                        .when_some(self.error.clone(), |d, e| d.child(div().mt_3().text_sm().text_color(theme.danger).child(e)))
                        .when(has_accounts, |d| d.child(section(tr!("ACCOUNTS", "KONTEN"))).child(accounts))
                        .child(section(tr!("ADD ACCOUNT", "KONTO HINZUFÜGEN")))
                        .child(add)
                        .child(section(tr!("COMMAND LINE", "KOMMANDOZEILE")))
                        .child(div().text_sm().text_color(theme.muted_foreground).child(tr!(
                            "The jm command and its MCP server (jm mcp) share these accounts. They can read, flag and file mail and write drafts, but cannot send: sending only happens here, from the draft.",
                            "Der Befehl jm und sein MCP-Server (jm mcp) nutzen dieselben Konten. Sie können lesen, markieren, ablegen und Entwürfe schreiben, aber nicht senden: gesendet wird nur hier, aus dem Entwurf."
                        ))),
                ),
            )
    }
}
