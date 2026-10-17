#!/usr/bin/env bash
# Build and serve on the isolated test network with a separate publisher key.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

for tool in cargo dx freenet fdev xz python3; do
  command -v "$tool" >/dev/null || { echo "Missing required tool: $tool" >&2; exit 1; }
done
python3 -c 'import cryptography' || { echo "Install Python cryptography for the local node harness." >&2; exit 1; }

cargo build --locked --release --target wasm32-unknown-unknown \
  -p game-contract -p lobby-contract -p archive-contract -p player-contract \
  -p web-container-contract -p chess-delegate

OUT=target/local-webapp
mkdir -p "$OUT"
cargo run --locked --quiet -p webapp-packager -- \
  --key "$OUT/publisher.key" --out-dir "$OUT" --params-only
KEY=$(fdev get-contract-id --code published-contract/web_container_contract.wasm \
  --parameters "$OUT/webapp.params" | tr -d '[:space:]')
test -n "$KEY"
printf '%s\n' "$KEY" > "$OUT/contract-id.txt"

rm -rf target/dx/chess-ui
FREECHESS_BUILD="$(git rev-parse --short HEAD)+local" \
  dx build --release --platform web --package chess-ui --locked \
  --base-path "v1/contract/web/$KEY"

DIST=target/dx/chess-ui/release/web/public
test -f "$DIST/index.html"
./scripts/copy-stockfish-assets.sh "$DIST"
tar -C "$DIST" -cJf "$PWD/$OUT/webapp.tar.xz" .
VERSION=$(( $(cat "$OUT/version" 2>/dev/null || echo 0) + 1 ))
cargo run --locked --quiet -p webapp-packager -- --key "$OUT/publisher.key" \
  --archive "$OUT/webapp.tar.xz" --version "$VERSION" --out-dir "$OUT"
cargo run --locked --quiet -p bootstrap-contracts -- --out-dir "$OUT"

if ! curl -sf http://127.0.0.1:7511/ >/dev/null \
  || ! curl -sf http://127.0.0.1:7512/ >/dev/null \
  || ! curl -sf http://127.0.0.1:7514/ >/dev/null; then
  ./scripts/nodes.sh up
fi
# Earlier nodes may accept this version before a later publication fails.
# Reserve it now so an edited retry cannot reuse an already published version.
printf '%s\n' "$VERSION" > "$OUT/version"
for port in 7511 7512 7514; do
  fdev --port "$port" publish --timeout 60 \
    --code target/wasm32-unknown-unknown/release/lobby_contract.wasm \
    --parameters "$OUT/lobby.params" contract --state "$OUT/lobby.state"
  fdev --port "$port" publish --timeout 60 \
    --code published-contract/web_container_contract.wasm \
    --parameters "$OUT/webapp.params" contract --state "$OUT/webapp.state"
done
printf '\nFreeChess: http://127.0.0.1:7511/v1/contract/web/%s/\n' "$KEY"
printf 'Second player: http://127.0.0.1:7512/v1/contract/web/%s/\n' "$KEY"
