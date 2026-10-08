//! `jm`: Just Mail on the command line. Reads Microsoft 365 mail and prepares drafts for the
//! Just Mail app; it never sends. `jm mcp` offers the same operations as an MCP server.

mod mcp;
mod ops;
mod output;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::FalseyValueParser;
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use jm_core::{Error, ErrorCode, FlagStatus, Result};
use serde_json::{Value, json};

use crate::ops::{Ctx, DraftParams, ListParams};
use crate::output::View;

const MAIN_EXAMPLES: &str = "\
Examples:
  jm list                              newest 25 in the inbox
  jm list --unread --since 2d          unread mail of the last two days
  jm list --flagged                    flagged mail anywhere in the mailbox
  jm search \"Rechnung\" -n 10          full-text search
  jm show a1b2c3d4                     read one message (short id from a listing)
  jm drafts reply a1b2c3d4 --body \"Danke, passt so.\"
  jm --json list -a invoice -n 5       one JSON object on stdout

Message ids: listings show 8-character short ids; any unique prefix or the full Graph id works.
Exit codes: 0 ok, 1 general/Graph, 2 auth, 3 validation, 4 not found, 5 network.
Sending is not possible here: drafts are reviewed and sent in the Just Mail app.
Config, sign-ins and the short id index live in ~/.config/just-mail (JUST_MAIL_HOME moves it).";

#[derive(Parser, Debug)]
#[command(
    name = "jm",
    version,
    about = "Just Mail on the command line: read Microsoft 365 mail and prepare drafts (never sends)",
    after_help = MAIN_EXAMPLES,
    propagate_version = true
)]
struct Cli {
    /// Print exactly one JSON object on stdout (errors too)
    #[arg(long, global = true, env = "JM_JSON", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new())]
    json: bool,

    /// Account id or address [default: the first account, or the account of a short id]
    #[arg(short, long, global = true, env = "JM_ACCOUNT", value_name = "ID|EMAIL")]
    account: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List accounts, or add, import or remove one
    #[command(
        after_help = "Examples:\n  jm accounts\n  jm accounts import     take over the ms365-mail sign-ins\n  jm accounts add        sign in with a device code"
    )]
    Accounts {
        #[command(subcommand)]
        action: Option<AccountsCommand>,
    },

    /// Folder tree with unread/total counts
    Folders,

    /// List messages, newest first (default: inbox)
    #[command(
        after_help = "Examples:\n  jm list -n 50\n  jm list -f archive --since 2026-10-01\n  jm list --unread --attachments\n  jm list --from rechnung@example.com --all\n  jm list --flagged              (whole mailbox unless -f is given)"
    )]
    List(ListArgs),

    /// Full-text search over subject, body and people (whole mailbox unless -f)
    #[command(after_help = "Examples:\n  jm search \"Angebot Fenster\"\n  jm search spark -f inbox -n 5")]
    Search {
        query: String,
        /// Only this folder
        #[arg(short, long)]
        folder: Option<String>,
        /// How many results
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: u32,
    },

    /// Show one message: headers, readable text, attachments
    #[command(
        after_help = "Examples:\n  jm show a1b2c3d4\n  jm show a1b2 --html > mail.html\n  jm show a1b2c3d4 --raw"
    )]
    Show {
        /// Message id (short id, prefix or full id)
        id: String,
        /// Print the HTML body instead of text
        #[arg(long, conflicts_with = "raw")]
        html: bool,
        /// Print the whole Graph message as JSON
        #[arg(long)]
        raw: bool,
    },

    /// List a message's attachments
    Attachments {
        /// Message id
        id: String,
    },

    /// Save attachments (default: all files except inline images) and print the paths
    #[command(
        after_help = "Examples:\n  jm download a1b2c3d4\n  jm download a1b2c3d4 Rechnung.pdf -o ~/Desktop\n\nExisting files are kept: a second copy becomes \"file (2).pdf\"."
    )]
    Download {
        /// Message id
        id: String,
        /// Attachment names (default: all file attachments)
        names: Vec<String>,
        /// Target folder [default: ~/Downloads]
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },

    /// Flag a message (or mark the flag done, or clear it)
    Flag {
        /// Message id
        id: String,
        /// Mark the flag complete
        #[arg(long, conflicts_with = "clear")]
        done: bool,
        /// Remove the flag
        #[arg(long)]
        clear: bool,
    },

    /// Mark a message read
    Read {
        /// Message id
        id: String,
    },

    /// Mark a message unread
    Unread {
        /// Message id
        id: String,
    },

    /// Move a message to the archive
    Archive {
        /// Message id
        id: String,
    },

    /// Move a message to Deleted Items (recoverable)
    Trash {
        /// Message id
        id: String,
    },

    /// Move a message to a folder
    #[command(after_help = "Examples:\n  jm move a1b2c3d4 archive\n  jm move a1b2c3d4 \"Projekte\"")]
    Move {
        /// Message id
        id: String,
        /// inbox, archive, trash, junk, a folder name or id
        folder: String,
    },

    /// Drafts: list, show, create, reply, forward, update, delete (never send)
    Drafts {
        #[command(subcommand)]
        action: Option<DraftsCommand>,
    },

    /// Show a message in the Just Mail app (starts it if needed)
    Open {
        /// Message id
        id: String,
    },

    /// Run the MCP server on stdio (for agents)
    #[command(
        after_help = "Add to an MCP client as a stdio server with the command `jm mcp`.\nTools: list_accounts, list_folders, list_messages, search_messages, get_message,\ndownload_attachments, set_flag, mark_read, move_message, list_drafts, create_draft,\nreply_draft, forward_draft, update_draft, delete_draft. None of them sends mail."
    )]
    Mcp,

    /// Not available: sending happens in the Just Mail app
    #[command(hide = true)]
    Send {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum AccountsCommand {
    /// Sign in with a device code (prints URL and code, copies the code)
    Add,
    /// Take over the sign-ins of ms365-mail (all profiles)
    Import,
    /// Remove an account and its sign-in
    Remove {
        /// Account id or address
        id: String,
    },
}

#[derive(Args, Debug)]
struct ListArgs {
    /// Folder: inbox, drafts, sent, archive, trash, junk, a folder name or id [default: inbox]
    #[arg(short, long, conflicts_with = "all")]
    folder: Option<String>,
    /// How many messages
    #[arg(short = 'n', long, default_value_t = 25)]
    limit: u32,
    /// Only unread
    #[arg(long)]
    unread: bool,
    /// Only flagged (whole mailbox unless -f is given)
    #[arg(long)]
    flagged: bool,
    /// Only with attachments
    #[arg(long)]
    attachments: bool,
    /// Only from this address
    #[arg(long, value_name = "ADDRESS")]
    from: Option<String>,
    /// Only newer than: 36h, 7d, 2w, today, yesterday, 2026-10-01 or an ISO time
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,
    /// Whole mailbox (all folders)
    #[arg(long)]
    all: bool,
    /// Continue a listing: the `next` value of an earlier --json result
    #[arg(long, value_name = "URL", conflicts_with_all = ["folder", "all"])]
    next: Option<String>,
}

#[derive(Subcommand, Debug)]
enum DraftsCommand {
    /// List drafts, newest first (also: `jm drafts`)
    List {
        /// How many drafts
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: u32,
    },
    /// Show a draft, with the author's text separated from signature and quote
    Show {
        /// Draft id
        id: String,
    },
    /// Create a new draft (with the account signature)
    #[command(
        after_help = "Examples:\n  jm drafts create --to max@example.com --subject \"Angebot\" --body \"Hallo Max, anbei das Angebot.\"\n  jm drafts create --to \"Max <max@example.com>, eva@example.com\" --subject Hi --body-file text.txt --attach angebot.pdf\n  echo \"Hallo\" | jm drafts create --to max@example.com --subject Hi --body-file -"
    )]
    Create {
        /// Recipients (repeat or comma-separate; "Name <address>" works)
        #[arg(long, required = true, value_name = "ADDRS")]
        to: Vec<String>,
        #[command(flatten)]
        copies: CopyArgs,
        /// Subject line
        #[arg(long)]
        subject: String,
        #[command(flatten)]
        body: BodyArgs,
        /// Importance
        #[arg(long, value_enum)]
        importance: Option<Importance>,
    },
    /// Create a reply draft: your text and signature above the quoted mail
    #[command(
        after_help = "Examples:\n  jm drafts reply a1b2c3d4 --body \"Danke, passt so.\"\n  jm drafts reply a1b2c3d4 --all --body-file antwort.txt --attach plan.pdf"
    )]
    Reply {
        /// Message to answer
        id: String,
        /// Reply to all recipients
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        body: BodyArgs,
    },
    /// Create a forward draft (keeps the original attachments)
    #[command(after_help = "Example:\n  jm drafts forward a1b2c3d4 --to eva@example.com --body \"Zur Info.\"")]
    Forward {
        /// Message to forward
        id: String,
        /// Recipients (repeat or comma-separate)
        #[arg(long, required = true, value_name = "ADDRS")]
        to: Vec<String>,
        #[command(flatten)]
        body: OptionalBodyArgs,
    },
    /// Change a draft; a new body replaces only your text (signature and quote stay)
    #[command(
        after_help = "Examples:\n  jm drafts update a1b2c3d4 --body \"Neuer Text\"\n  jm drafts update a1b2c3d4 --subject \"Re: Angebot v2\" --attach v2.pdf\n\nRecipients given replace the old ones; attachments are added."
    )]
    Update {
        /// Draft id
        id: String,
        /// New recipients
        #[arg(long, value_name = "ADDRS")]
        to: Vec<String>,
        #[command(flatten)]
        copies: CopyArgs,
        /// New subject
        #[arg(long)]
        subject: Option<String>,
        /// New text (plain text unless --html)
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the new text from a file (- = stdin)
        #[arg(long, value_name = "PATH")]
        body_file: Option<PathBuf>,
        /// The body is HTML
        #[arg(long)]
        html: bool,
        /// Attach a file (repeatable)
        #[arg(long, value_name = "FILE")]
        attach: Vec<PathBuf>,
        /// Importance
        #[arg(long, value_enum)]
        importance: Option<Importance>,
    },
    /// Delete a draft for good (drafts only)
    Delete {
        /// Draft id
        id: String,
    },
    /// Not available: sending happens in the Just Mail app
    #[command(hide = true)]
    Send {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Args, Debug)]
struct CopyArgs {
    /// Cc recipients
    #[arg(long, value_name = "ADDRS")]
    cc: Vec<String>,
    /// Bcc recipients
    #[arg(long, value_name = "ADDRS")]
    bcc: Vec<String>,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false, id = "body_source")]
struct BodyArgs {
    /// Your text (plain text unless --html)
    #[arg(long, group = "body_source")]
    body: Option<String>,
    /// Read your text from a file (- = stdin)
    #[arg(long, value_name = "PATH", group = "body_source")]
    body_file: Option<PathBuf>,
    #[command(flatten)]
    extra: BodyExtras,
}

#[derive(Args, Debug)]
struct OptionalBodyArgs {
    /// Your text (plain text unless --html)
    #[arg(long, conflicts_with = "body_file")]
    body: Option<String>,
    /// Read your text from a file (- = stdin)
    #[arg(long, value_name = "PATH")]
    body_file: Option<PathBuf>,
    #[command(flatten)]
    extra: BodyExtras,
}

#[derive(Args, Debug)]
struct BodyExtras {
    /// The body is HTML (default: plain text)
    #[arg(long)]
    html: bool,
    /// Attach a file (repeatable)
    #[arg(long, value_name = "FILE")]
    attach: Vec<PathBuf>,
    /// Leave out the account signature
    #[arg(long)]
    no_signature: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum Importance {
    Low,
    Normal,
    High,
}

impl Importance {
    fn as_str(self) -> &'static str {
        match self {
            Importance::Low => "low",
            Importance::Normal => "normal",
            Importance::High => "high",
        }
    }
}

impl BodyExtras {
    fn into_params(self, body: Option<String>, body_file: Option<PathBuf>) -> DraftParams {
        DraftParams {
            body,
            body_file,
            html: self.html,
            attachments: self.attach,
            signature: !self.no_signature,
            ..Default::default()
        }
    }
}

/// What a command produced and how to show it to people.
type Outcome = (View, Value);

fn run(cli: Cli) -> Result<Outcome> {
    let account = cli.account;
    let ctx = || Ctx::load(account.clone());
    Ok(match cli.command {
        Command::Accounts { action } => match action {
            None => (View::Accounts, ops::accounts(&jm_core::Config::load()?)),
            Some(AccountsCommand::Add) => (View::AccountChange, ops::accounts_add()?),
            Some(AccountsCommand::Import) => (View::AccountChange, ops::accounts_import()?),
            Some(AccountsCommand::Remove { id }) => (View::AccountChange, ops::accounts_remove(&id)?),
        },
        Command::Folders => (View::Folders, ops::folders(&ctx()?)?),
        Command::List(a) => {
            let params = ListParams {
                folder: a.folder,
                limit: a.limit,
                unread: a.unread,
                flagged: a.flagged,
                attachments: a.attachments,
                from: a.from,
                since: a.since,
                all: a.all,
                next: a.next,
            };
            (View::Messages, ops::list(&ctx()?, &params)?)
        }
        Command::Search { query, folder, limit } => {
            (View::Messages, ops::search(&ctx()?, &query, folder.as_deref(), limit)?)
        }
        Command::Show { id, html, raw } => {
            let view = if raw {
                View::Raw
            } else if html {
                View::Html
            } else {
                View::Message
            };
            (view, ops::show(&ctx()?, &id, html, raw)?)
        }
        Command::Attachments { id } => (View::Attachments, ops::attachments(&ctx()?, &id)?),
        Command::Download { id, names, output } => {
            (View::Saved, ops::download(&ctx()?, &id, &names, output.as_deref())?)
        }
        Command::Flag { id, done, clear } => {
            let status = if done {
                FlagStatus::Complete
            } else if clear {
                FlagStatus::NotFlagged
            } else {
                FlagStatus::Flagged
            };
            (View::Changed, ops::set_flag(&ctx()?, &id, status)?)
        }
        Command::Read { id } => (View::Changed, ops::mark_read(&ctx()?, &id, true)?),
        Command::Unread { id } => (View::Changed, ops::mark_read(&ctx()?, &id, false)?),
        Command::Archive { id } => (View::Changed, ops::move_message(&ctx()?, &id, "archive")?),
        Command::Trash { id } => (View::Changed, ops::move_message(&ctx()?, &id, "trash")?),
        Command::Move { id, folder } => (View::Changed, ops::move_message(&ctx()?, &id, &folder)?),
        Command::Drafts { action } => drafts(ctx, action.unwrap_or(DraftsCommand::List { limit: 25 }))?,
        Command::Open { id } => (View::Opened, ops::open(&ctx()?, &id)?),
        Command::Mcp => return Err(Error::general("`jm mcp` runs as a server").hint("start it with `jm mcp`")),
        Command::Send { .. } => return Err(ops::refuse_send()),
    })
}

fn drafts(ctx: impl Fn() -> Result<Ctx>, action: DraftsCommand) -> Result<Outcome> {
    Ok(match action {
        DraftsCommand::List { limit } => (View::Messages, ops::list_drafts(&ctx()?, limit)?),
        DraftsCommand::Show { id } => (View::Message, ops::show(&ctx()?, &id, false, false)?),
        DraftsCommand::Create { to, copies, subject, body, importance } => {
            let params = DraftParams {
                to,
                cc: copies.cc,
                bcc: copies.bcc,
                subject: Some(subject),
                importance: importance.map(|i| i.as_str().to_string()),
                ..body.extra.into_params(body.body, body.body_file)
            };
            (View::Draft, ops::create_draft(&ctx()?, &params)?)
        }
        DraftsCommand::Reply { id, all, body } => {
            let params = body.extra.into_params(body.body, body.body_file);
            (View::Draft, ops::reply_draft(&ctx()?, &id, all, &params)?)
        }
        DraftsCommand::Forward { id, to, body } => {
            let params = DraftParams { to, ..body.extra.into_params(body.body, body.body_file) };
            (View::Draft, ops::forward_draft(&ctx()?, &id, &params)?)
        }
        DraftsCommand::Update { id, to, copies, subject, body, body_file, html, attach, importance } => {
            let params = DraftParams {
                to,
                cc: copies.cc,
                bcc: copies.bcc,
                subject,
                body,
                body_file,
                html,
                attachments: attach,
                signature: true,
                importance: importance.map(|i| i.as_str().to_string()),
            };
            (View::Draft, ops::update_draft(&ctx()?, &id, &params)?)
        }
        DraftsCommand::Delete { id } => (View::Deleted, ops::delete_draft(&ctx()?, &id)?),
        DraftsCommand::Send { .. } => return Err(ops::refuse_send()),
    })
}

/// Same reading as clap's `FalseyValueParser`: set and not `0`/`false`/`no`/`off`/`n`/`f`/empty.
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !matches!(v.trim().to_lowercase().as_str(), "" | "0" | "false" | "no" | "off" | "n" | "f"))
        .unwrap_or(false)
}

/// Write to stdout; a reader that went away (`jm list | head`) is not an error.
fn print_out(text: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

fn print_error(e: &Error, json: bool) {
    if json {
        print_out(&format!("{}\n", json!({ "error": e })));
    } else {
        eprintln!("error: {}", e.message);
        if let Some(hint) = &e.hint {
            eprintln!("hint: {hint}");
        }
    }
}

/// A clap usage error as our validation error: first line is the message, tips are the hint.
fn usage_error(e: &clap::Error) -> Error {
    let rendered = e.render().to_string();
    let mut lines = rendered.lines().map(str::trim).filter(|l| !l.is_empty());
    let message = lines.next().unwrap_or("invalid arguments").trim_start_matches("error: ").to_string();
    let mut hints: Vec<String> = lines
        .filter(|l| l.starts_with("tip:") || l.starts_with("Usage:"))
        .map(|l| l.trim_start_matches("tip: ").to_string())
        .collect();
    hints.push("see `jm <command> --help`".into());
    Error::validation(message).hint(hints.join("; "))
}

fn exit(code: ErrorCode) -> ExitCode {
    ExitCode::from(code.exit_code() as u8)
}

fn main() -> ExitCode {
    let json_asked = std::env::args().any(|a| a == "--json") || env_flag("JM_JSON");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    let _ = e.print();
                    return ExitCode::SUCCESS;
                }
                _ if json_asked => print_error(&usage_error(&e), true),
                _ => {
                    let _ = e.print();
                }
            }
            return exit(ErrorCode::Validation);
        }
    };
    if matches!(cli.command, Command::Mcp) {
        return match mcp::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                print_error(&e, false);
                exit(e.code)
            }
        };
    }
    let json = cli.json;
    match run(cli) {
        Ok((view, value)) => {
            if json {
                print_out(&format!("{value}\n"));
            } else {
                print_out(&output::render(view, &value));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_error(&e, json);
            exit(e.code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn send_is_refused_with_a_validation_error() {
        let attempts: [&[&str]; 3] =
            [&["jm", "send", "a1b2c3d4"], &["jm", "send", "--to", "x@y.z"], &["jm", "drafts", "send", "a1b2"]];
        for args in attempts {
            let err = run(parse(args)).unwrap_err();
            assert_eq!(err.code, ErrorCode::Validation);
            assert_eq!(err.code.exit_code(), 3);
            assert_eq!(err.message, ops::SEND_REFUSAL);
        }
    }

    #[test]
    fn send_does_not_show_up_in_help() {
        let help = <Cli as clap::CommandFactory>::command().render_help().to_string();
        assert!(!help.contains("\n  send"), "{help}");
    }

    #[test]
    fn global_flags_work_after_the_command() {
        let cli = parse(&["jm", "list", "-n", "5", "--json", "-a", "invoice"]);
        assert!(cli.json);
        assert_eq!(cli.account.as_deref(), Some("invoice"));
    }

    #[test]
    fn create_needs_recipients_subject_and_a_body() {
        assert!(Cli::try_parse_from(["jm", "drafts", "create", "--subject", "x", "--body", "y"]).is_err());
        assert!(Cli::try_parse_from(["jm", "drafts", "create", "--to", "a@b.c", "--subject", "x"]).is_err());
        let both = ["jm", "drafts", "create", "--to", "a@b.c", "--subject", "x", "--body", "y", "--body-file", "z"];
        assert!(Cli::try_parse_from(both).is_err());
        parse(&["jm", "drafts", "create", "--to", "a@b.c", "--subject", "x", "--body", "y", "--no-signature"]);
    }

    #[test]
    fn typos_become_validation_errors_with_suggestions() {
        let err = Cli::try_parse_from(["jm", "serch", "x"]).unwrap_err();
        let ours = usage_error(&err);
        assert_eq!(ours.code, ErrorCode::Validation);
        assert!(ours.hint.unwrap().contains("search"));
    }
}
