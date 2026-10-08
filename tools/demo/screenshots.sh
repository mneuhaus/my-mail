#!/bin/sh
# Screenshots for the GitHub page (docs/), taken from the demo mailbox only, never from real mail.
# Usage: tools/demo/screenshots.sh   (needs release builds of app and jm: tools/bundle-macos.sh)
set -eu
cd "$(dirname "$0")/../.."
APP=target/release/just-mail
OUT=docs
mkdir -p "$OUT"

# Run one demo app, open message $2, optionally start a reply, capture the window to $3.
shot() {
    theme=$1 message=$2 out=$3 action=${4:-}
    DEMO_PORT=7358 JUST_MAIL_THEME=$theme JUST_MAIL_LANG=en tools/demo/run.sh "$APP" >/dev/null 2>&1 &
    runner=$!
    sleep 4
    events=tmp/demo-home/events.jsonl
    printf '{"kind":"open","account":"alex","id":"%s"}\n' "$message" >> "$events"
    sleep 2
    if [ -n "$action" ]; then
        # a real reply through jm against the demo mailbox: text, signature, quote
        jm() { JUST_MAIL_HOME="$PWD/tmp/demo-home" JUST_MAIL_GRAPH_URL=http://127.0.0.1:7358/v1.0 target/release/jm "$@"; }
        original=$(jm --json search "website relaunch" | jq -r '.messages[] | select(.subject | startswith("Re:")) | .id' | head -1)
        printf 'Hi Tom,\n\nhosting for the first year is included, and the 40 product pages are covered as well.\n\nNovember 3 works for us, I am looking forward to it.\n' > tmp/demo-reply.txt
        draft=$(jm --json drafts reply "$original" --body-file tmp/demo-reply.txt | jq -r .draft.id)
        jm open "$draft" > /dev/null
        sleep 2
    fi
    pid=$(pgrep -n -f "$APP")
    wid=$(swift tools/winid.swift just-mail "$pid")
    screencapture -x -o -l "$wid" "$out"
    kill "$pid" 2>/dev/null || true
    wait "$runner" 2>/dev/null || true
}

shot dark msg-2 "$OUT/screenshot-dark.png"
shot light msg-2 "$OUT/screenshot-light.png"
shot dark msg-2 "$OUT/screenshot-reply.png" reply
for f in "$OUT"/screenshot-*.png; do sips -Z 2400 "$f" >/dev/null; done
ls -la "$OUT"
