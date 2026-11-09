#!/bin/sh
# What the draft editor's page does, in WebKit with real keys and clicks, without the app's window:
# formats, lists, links, tables, pictures, undo. The app only prints the page (JUST_MAIL_EDITOR_PAGE).
# Usage: tools/editor-test/run.sh [part of a test's name]
# The first run fetches Playwright and its WebKit (pnpm, about 80 MB, into the user's cache).
set -eu
cd "$(dirname "$0")"
cargo build --quiet -p jm-app
[ -d node_modules ] || pnpm install --silent
pnpm exec playwright-core install webkit >/dev/null
node test.mjs "$@"
