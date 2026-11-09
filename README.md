# Just Mail

A no-nonsense Microsoft 365 mail client for macOS: a Rust/GPUI desktop app and a CLI (`jm`, with
an MCP server) that share the same accounts. Mail goes through Microsoft Graph only.

- **Several accounts**, each with its own folders and signature. Accounts can be **read-only**
  (no marking as read, flags, moves or drafts), for mailboxes another system processes.
- **Read**: mail shows in WebKit, the way its sender designed it (columns, colors, fonts), like in
  Apple Mail or Outlook. Designed mail stays on light paper in dark mode; plain mail takes the
  app's colors. Nothing in a mail runs (a content policy allows only the app's own script), and
  nothing is loaded from elsewhere (no tracking pixels, remote images only on request, no link
  previews, no cookies or cache kept between mails). An attachment you open is saved with the
  quarantine mark, so macOS asks before it runs one. Pictures
  sent along with the mail (logos in signatures) show right away; hidden preview text stays
  hidden. Below a mail comes what followed in its conversation, your own answers and drafts
  included, each with only its new part (Graph's `uniqueBody`).
- **Unread count on the Dock icon**: unread mail in the inboxes (read-only accounts stay out),
  current within a minute.
- **Pin** mail you still have to deal with (Spark's pin, stored as the Outlook follow-up flag, so
  Outlook and phones see it too). Pinned mail sits on top of the inbox; *Pinned* lists it for all
  accounts. Right-click a mail for reply, pin, read/unread, archive, delete or discard (drafts).
  In the trash, *Move to inbox* (right-click, or the reader's toolbar) takes a mail back.
- **Drafts live on the server.** New mail, replies (in the conversation, above the quote) and
  forwards are Graph drafts; the editor saves while you type, and what you typed last when you
  open another mail or quit. Files attached to a draft open with a click, also when an agent
  attached them. Your signature is added once and kept out of the text,
  so editing never mangles it or the quote. Two HTML signatures per account (taken from Spark): the
  long one for new mails, the short one for replies and forwards; logos go along as inline images.
- **Rich text**: the editor is WebKit, so what a draft carries stays as it is, whoever wrote it:
  bold, lists, tables, links, pictures. A format bar above the text has text size, bold, italic,
  underlined, struck through, text color and marker, lists, quote, link, picture, table and
  "remove formatting"; its buttons show what holds where the cursor is. `⌘B`, `⌘I`, `⌘U`, `⇧⌘X`,
  `⌘K` links what is selected (a typed or pasted web address becomes a link by itself, `⌘` and a
  click open one), `⇧⌘8` bullets and `⇧⌘7` numbers (or `- ` and `1. ` at the start of a line),
  `⇥`/`⇧⇥` move list items in and out and walk the cells of a table, `⌘Z`/`⇧⌘Z`.
- **Pictures** go into the text where the cursor is and out as inline images: paste one with
  `⌘V` (a screenshot, say), drop files on the text, or pick them with the picture button. A click
  on a picture shows a corner to drag its size with. An iPhone photo (HEIC) becomes a JPEG, and a
  photo taken sideways is turned upright. Other files dropped or pasted become attachments; text
  copied from a web page or Word keeps its structure but not its fonts and colors (`⇧⌘V` pastes
  text only).
- **Recipients are chips**: ⌫ takes out a whole recipient, a click shows its address. A chip
  shows the name alone for people your mail knows under that name; a recipient a draft came with
  that it never saw shows the address too. Typing
  suggests people from your mail, those you write to first (the recipients of the last 1000 sent
  mails, the senders of the last 500 in the inbox); `↑/↓` pick, `↩`, `⇥` or a comma take one. A
  typed address becomes a chip at a comma, with `↩` or when you leave the field. Only the
  Mail scopes are needed, so no new consent.
- **Sending only in the app**, from the draft (Send or ⌘↩). The CLI and its MCP server are
  built without the sending code (`jm-core` feature `send`, enabled only for the app): agents can
  prepare drafts, a person sends them. And what goes out is what that person saw: right before
  sending, the draft on the server is compared with what the window showed (text, subject,
  recipients, files). If `jm` or another program changed it meanwhile, nothing is sent; the
  window shows the new state to be read and sent again. An open draft that `jm` changes shows
  the change by itself.

## CLI

```
jm list [--unread] [--flagged] [-f folder] [--since 2d]
jm search "Rechnung" [--since 2w]
jm show a1b2c3d4                       # 8-character short ids from listings
jm drafts reply a1b2c3d4 --body "Passt so, danke!"
jm drafts reply a1b2c3d4 --markdown --body-file antwort.md   # **bold**, lists, tables
jm drafts create --to max@example.com --subject "Angebot" --body-file mail.txt
jm flag a1b2c3d4 | jm archive a1b2c3d4
jm open a1b2c3d4                       # show it in the app
jm mcp                                 # MCP server on stdio
```

Draft text is plain text by default; `--markdown` takes Markdown (`**bold**`, lists, tables,
links) and turns it into mail HTML that Outlook shows as it should (tables with borders and a
header row), `--html` takes HTML as it is. The MCP tools have `markdown` and `html` for the same.

`--json` prints exactly one JSON object. `-a <id|address>` picks the account (default: the first).
Exit codes: 0 ok, 1 general/Graph, 2 auth, 3 validation, 4 not found, 5 network.

The CLI tells a running app what changed (`~/.config/just-mail/events.jsonl`), so a draft written by
an agent shows up right away. `Y` in the app copies the short id of the open mail.

Through MCP, attachments are saved to `~/Downloads` or a folder the agent names, but never into
hidden folders or `~/Library`, where a file from a mail could become something that runs. What
`jm` prints for a terminal has no control characters (a mail cannot steer the terminal).

## Setup

Sign in from *Accounts & signatures* in the app (Microsoft device code), or take over the sign-ins of
`ms365-mail` (same app registration, no new consent): button in the app or `jm accounts import`.

Files: `~/.config/just-mail/config.toml` (accounts, signatures, read-only), `accounts/<id>/token.json`
(mode 0600). `JUST_MAIL_HOME` moves everything elsewhere.

## Keys

`↑/↓` or `j/k` move, `r` reply, `a` reply all, `f` forward, `c`/`⌘N` new, `s` pin, `u` read/unread,
`e` archive, `⌫` trash, `y` copy id, `/` or `⌘F` search, `⇧⌘N` check mail; in a draft `⌘↩` send and
`⌘S` save. `⌘R` reloads everything, as in a browser: settings and sign-ins, folders, the list,
recipient suggestions and the open mail; an open draft too, unless edits are still on their way.

## Build

```
tools/bundle-macos.sh            # dist/Just Mail.app and dist/jm
tools/bundle-macos.sh --install  # plus /Applications and ~/bin/jm
swift tools/make-icon.swift      # redraw the icon
```

Crates: `jm-core` (accounts, sign-in, Graph, draft HTML), `jm-cli` (`jm`), `jm-app` (GPUI).

Try the app without an account: `tools/demo/run.sh` starts it on a made-up mailbox (a small local
stand-in for Graph, `tools/demo/server.mjs`); `tools/demo/screenshots.sh` makes the pictures for the
page in `docs/` (GitHub Pages).

`cargo test` runs the unit tests. The draft editor's text is a web page (`crates/jm-app/src/editor/`);
`tools/editor-test/run.sh` runs it in WebKit without the app's window and checks what it does with
real keys and clicks (formats, lists, links, tables, pictures, undo). It needs Node and pnpm, and
fetches Playwright's WebKit on its first run.

This repo is public. After cloning, turn on the checks that keep secrets and local files out of
commits and pushes (they use [gitleaks](https://github.com/gitleaks/gitleaks), `brew install gitleaks`):

```
git config core.hooksPath .githooks
```
