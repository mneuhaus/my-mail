#!/bin/sh
# Start Just Mail against the made-up demo mailbox (tools/demo/server.mjs) instead of Microsoft.
# Usage: tools/demo/run.sh [app binary]   e.g. tools/demo/run.sh target/release/just-mail
# Environment passes through (JUST_MAIL_THEME, JUST_MAIL_LANG, …). Your real accounts stay untouched:
# the demo lives in tmp/demo-home.
set -eu
cd "$(dirname "$0")/../.."
APP=${1:-target/release/just-mail}
PORT=${DEMO_PORT:-7357}
HOME_DIR="$PWD/tmp/demo-home"

rm -rf "$HOME_DIR"
mkdir -p "$HOME_DIR/accounts/alex" "$HOME_DIR/accounts/billing"
cat > "$HOME_DIR/config.toml" <<'TOML'
load_remote_images = false

[[account]]
id = "alex"
email = "alex@northwind.example"
name = "Alex Morgan"
signature = "Alex Morgan\nNorthwind Studio\nnorthwind.example"

[[account]]
id = "billing"
email = "billing@northwind.example"
name = "Billing"
read_only = true
TOML
# tokens that never expire, so nothing ever talks to Microsoft's sign-in
for account in alex billing; do
    printf '{"access_token":"demo-%s","refresh_token":"demo","expires_at":4102444800}\n' "$account" \
        > "$HOME_DIR/accounts/$account/token.json"
done

node tools/demo/server.mjs "$PORT" &
SERVER=$!
trap 'kill $SERVER 2>/dev/null' EXIT INT TERM
sleep 0.5

JUST_MAIL_HOME="$HOME_DIR" JUST_MAIL_GRAPH_URL="http://127.0.0.1:$PORT/v1.0" "$APP"
