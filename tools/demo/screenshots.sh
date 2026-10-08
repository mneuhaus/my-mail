#!/bin/sh
# Screenshots for the GitHub page (docs/), taken from the demo mailbox only, never from real mail.
# Usage: tools/demo/screenshots.sh   (needs a release build: cargo build --release -p jm-app)
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
        demo_auth="Authorization: Bearer demo-alex" # the demo server's fixed token, not a secret
        # the app reacts to Open events only; a reply draft is created through the demo server
        curl -s -X POST -H "$demo_auth" "http://127.0.0.1:7358/v1.0/me/messages/$message/createReply" > /tmp/jm-demo-draft.json
        draft=$(sed 's/.*"id":"\(msg-[0-9]*\)".*/\1/' /tmp/jm-demo-draft.json | head -1)
        curl -s -X PATCH -H "$demo_auth" -H "content-type: application/json" \
            -d '{"body":{"content":"<html><body><div id=\"jm-body\">Hi Tom,<br><br>hosting for the first year is included, and the 40 product pages are covered as well.<br><br>November 3 works for us, I am looking forward to it.</div><div id=\"jm-signature\"><br>Alex Morgan<br>Northwind Studio<br>northwind.example</div></body></html>"}}' \
            "http://127.0.0.1:7358/v1.0/me/messages/$draft" > /dev/null
        printf '{"kind":"open","account":"alex","id":"%s"}\n' "$draft" >> "$events"
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
