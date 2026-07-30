#!/usr/bin/env bash
# Isolated two-node Freenet network for testing FreeChess.
#
# Why the isolation is fussy: `--data-dir` does NOT isolate everything. The
# gateway bootstrap list lives in the global config dir, so a "local" node will
# happily dial real public gateways and join the live network. And two nodes
# sharing a config dir share `config.toml` and the transport keypair, so the
# second one fails to start or collides with the first.
#
# So each node gets its own `--config-dir` (the only flag that also wins against
# XDG_CONFIG_HOME on CI), and the gateway gets an empty `gateways.toml`.
#
# Both nodes also run with --disable-auto-update. A bare `freenet network` exits
# with code 42 the moment it notices a newer release, and with no supervisor to
# catch that it simply dies — mid-test, looking exactly like a crash. These are
# throwaway test nodes, so staying pinned to the installed version is correct;
# a real deployment must NOT set this flag.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${FREECHESS_TEST_DIR:-$ROOT/.freenet-test}"

# Deliberately NOT 7509/7510: a developer very likely already has a node on
# 7509, and clobbering it would take down whatever else they are running.
GW_WS=7511
GW_NET=31347
PEER_WS=7512
PEER_NET=31348
# A third node, because identity is per-NODE: the delegate holds the account, so
# two browser contexts on one node are the same player. Testing a spectator
# alongside two players therefore needs three nodes, not three tabs.
PEER2_WS=7514
PEER2_NET=31350

gw_dir="$BASE/gateway"
peer_dir="$BASE/peer"
peer2_dir="$BASE/peer2"

start_gateway() {
  mkdir -p "$gw_dir/config" "$gw_dir/data" "$gw_dir/logs"
  # An empty gateway list is what keeps this node off the public network.
  # The file must contain `gateways = []`; a truly empty file fails to parse.
  printf 'gateways = []\n' > "$gw_dir/config/gateways.toml"

  echo "starting gateway  ws=$GW_WS net=$GW_NET"
  freenet network \
    --is-gateway \
    --disable-auto-update \
    --skip-load-from-network \
    --public-network-address 127.0.0.1 \
    --public-network-port "$GW_NET" \
    --network-port "$GW_NET" \
    --ws-api-port "$GW_WS" \
    --ws-api-address 0.0.0.0 \
    --config-dir "$gw_dir/config" \
    --data-dir "$gw_dir/data" \
    --log-dir "$gw_dir/logs" \
    --log-level info \
    > "$gw_dir/stdout.log" 2>&1 &
  echo $! > "$gw_dir/pid"
}

gateway_pubkey() {
  # The peer dials the gateway with "ip:port,hex-pubkey", where the pubkey is
  # the gateway's X25519 transport *public* key. Only the private key is stored
  # on disk (64 hex chars in transport_keypair), so derive the public half.
  local key_file="$gw_dir/data/secrets/transport_keypair"
  local tries=0
  while [ ! -f "$key_file" ]; do
    tries=$((tries + 1))
    if [ "$tries" -gt 30 ]; then
      echo "gateway never wrote $key_file" >&2
      return 1
    fi
    sleep 1
  done

  python3 - "$key_file" <<'PY'
import sys
try:
    from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
    from cryptography.hazmat.primitives import serialization
except ImportError:
    sys.exit("this harness needs python3 'cryptography' (pip install cryptography)")

with open(sys.argv[1]) as f:
    private = bytes.fromhex(f.read().strip())

public = X25519PrivateKey.from_private_bytes(private).public_key().public_bytes(
    serialization.Encoding.Raw, serialization.PublicFormat.Raw
)
print(public.hex())
PY
}

start_peer() {
  local dir="$1" ws="$2" net="$3" name="$4"
  mkdir -p "$dir/config" "$dir/data" "$dir/logs"
  printf 'gateways = []\n' > "$dir/config/gateways.toml"

  local pubkey
  pubkey="$(gateway_pubkey)"

  echo "starting $name  ws=$ws net=$net"
  freenet network \
    --skip-load-from-network \
    --disable-auto-update \
    --network-port "$net" \
    --ws-api-port "$ws" \
    --ws-api-address 0.0.0.0 \
    --gateway "127.0.0.1:$GW_NET,$pubkey" \
    --config-dir "$dir/config" \
    --data-dir "$dir/data" \
    --log-dir "$dir/logs" \
    --log-level info \
    > "$dir/stdout.log" 2>&1 &
  echo $! > "$dir/pid"
}

wait_for_port() {
  local port="$1" name="$2" dir="$3" tries=0
  until curl -sf "http://127.0.0.1:$port/" > /dev/null 2>&1; do
    # Fail fast if the process died rather than waiting out the full timeout.
    if [ -f "$dir/pid" ] && ! kill -0 "$(cat "$dir/pid")" 2>/dev/null; then
      echo "$name exited during startup:" >&2
      tail -20 "$dir/stdout.log" >&2
      return 1
    fi
    tries=$((tries + 1))
    if [ "$tries" -gt 90 ]; then
      echo "$name did not come up on port $port" >&2
      tail -20 "$dir/stdout.log" >&2
      return 1
    fi
    sleep 1
  done
  echo "$name is up on $port"
}

# Wait for a port to become free. Killing a node is not instant, and starting
# the replacement too early makes it fail with "Database already open" — the
# old process still holds the redb lock.
wait_for_port_free() {
  local port="$1" tries=0
  while curl -sf "http://127.0.0.1:$port/" > /dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -gt 30 ]; then
      echo "port $port is still in use by something else" >&2
      return 1
    fi
    sleep 1
  done
}

case "${1:-up}" in
  up)
    "$0" down > /dev/null 2>&1 || true
    wait_for_port_free "$GW_WS"
    wait_for_port_free "$PEER_WS"
    wait_for_port_free "$PEER2_WS"

    start_gateway
    wait_for_port "$GW_WS" gateway "$gw_dir"
    start_peer "$peer_dir" "$PEER_WS" "$PEER_NET" "peer "
    wait_for_port "$PEER_WS" peer "$peer_dir"
    start_peer "$peer2_dir" "$PEER2_WS" "$PEER2_NET" "peer2"
    wait_for_port "$PEER2_WS" peer2 "$peer2_dir"

    echo
    echo "gateway  http://127.0.0.1:$GW_WS/"
    echo "peer     http://127.0.0.1:$PEER_WS/"
    echo "peer2    http://127.0.0.1:$PEER2_WS/"
    ;;

  down)
    for d in "$gw_dir" "$peer_dir" "$peer2_dir"; do
      if [ -f "$d/pid" ]; then
        pid="$(cat "$d/pid")"
        kill "$pid" 2>/dev/null || true
        # Wait for it to actually exit: the redb lock is only released on
        # process death, and the replacement cannot start until then.
        for _ in $(seq 30); do
          kill -0 "$pid" 2>/dev/null || break
          sleep 1
        done
        kill -9 "$pid" 2>/dev/null || true
        rm -f "$d/pid"
      fi
    done
    echo "nodes stopped"
    ;;

  wipe)
    "$0" down
    rm -rf "$BASE"
    echo "test network wiped"
    ;;

  status)
    for pair in "gateway:$GW_WS" "peer:$PEER_WS" "peer2:$PEER2_WS"; do
      name="${pair%%:*}"; port="${pair##*:}"
      if curl -sf "http://127.0.0.1:$port/" > /dev/null 2>&1; then
        echo "$name: up on $port"
      else
        echo "$name: down"
      fi
    done
    ;;

  logs)
    tail -f "$gw_dir/logs"/*.log "$peer_dir/logs"/*.log "$peer2_dir/logs"/*.log
    ;;

  *)
    echo "usage: $0 {up|down|wipe|status|logs}" >&2
    exit 1
    ;;
esac
