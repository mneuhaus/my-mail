# Just Mail

A no-nonsense Microsoft 365 mail client for macOS: a Rust/GPUI desktop app and a CLI (`jm`, with
an MCP server) that share the same accounts. Mail goes through Microsoft Graph only.

- **Several accounts**, each with its own folders and signature. Accounts can be **read-only**
  (no marking as read, flags, moves or drafts), for mailboxes another system processes.
- **Read**: HTML mail is cleaned up for reading (no scripts, no tracking pixels: remote images only
  on request). Below a mail comes what followed in its conversation, your own answers and drafts
  included, each with only its new part (Graph's `uniqueBody`).
- **Pin** mail you still have to deal with (Spark's pin, stored as the Outlook follow-up flag, so
  Outlook and phones see it too). Pinned mail sits on top of the inbox; *Pinned* lists it for all
  accounts. Right-click a mail for reply, pin, read/unread, archive, delete or discard (drafts).
- **Drafts live on the server.** New mail, replies (in the conversation, above the quote) and
  forwards are Graph drafts; the editor saves while you type. Your signature is added once and kept
  out of the text field, so editing never mangles it or the quote. Two HTML signatures per account
  (taken from Spark): the long one for new mails, the short one for replies and forwards; logos go
  along as inline images.
- **Sending only in the app**, from the draft (Send or ⌘↩). The CLI and its MCP server are
  built without the sending code (`jm-core` feature `send`, enabled only for the app): agents can
  prepare drafts, a person sends them.

## CLI

```
jm list [--unread] [--flagged] [-f folder] [--since 2d]
jm search "Rechnung"
jm show a1b2c3d4                       # 8-character short ids from listings
jm drafts reply a1b2c3d4 --body "Passt so, danke!"
jm drafts create --to max@example.com --subject "Angebot" --body-file mail.txt
jm flag a1b2c3d4 | jm archive a1b2c3d4
jm open a1b2c3d4                       # show it in the app
jm mcp                                 # MCP server on stdio
```

`--json` prints exactly one JSON object. `-a <id|address>` picks the account (default: the first).
Exit codes: 0 ok, 1 general/Graph, 2 auth, 3 validation, 4 not found, 5 network.

The CLI tells a running app what changed (`~/.config/just-mail/events.jsonl`), so a draft written by
an agent shows up right away. `Y` in the app copies the short id of the open mail.

## Setup

Sign in from *Accounts & signatures* in the app (Microsoft device code), or take over the sign-ins of
`ms365-mail` (same app registration, no new consent): button in the app or `jm accounts import`.

Files: `~/.config/just-mail/config.toml` (accounts, signatures, read-only), `accounts/<id>/token.json`
(mode 0600). `JUST_MAIL_HOME` moves everything elsewhere.

## Keys

`↑/↓` or `j/k` move, `r` reply, `a` reply all, `f` forward, `c`/`⌘N` new, `s` pin, `u` read/unread,
`e` archive, `⌫` trash, `y` copy id, `/` or `⌘F` search, `⇧⌘N` check mail; in a draft `⌘↩` send and
`⌘S` save.

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

This repo is public. After cloning, turn on the checks that keep secrets and local files out of
commits and pushes (they use [gitleaks](https://github.com/gitleaks/gitleaks), `brew install gitleaks`):

```
git config core.hooksPath .githooks
```
