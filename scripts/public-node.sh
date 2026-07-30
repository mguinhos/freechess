#!/usr/bin/env bash
# Run a node that joins the REAL Freenet network, for publishing FreeChess so
# other people's nodes can reach it.
#
# This is the opposite of scripts/nodes.sh: no `gateways = []`, no
# --skip-load-from-network, so the node bootstraps against the public gateway
# index and becomes an ordinary participant.
#
# Anything published from here propagates to other peers and is not easily
# unpublished. That is the point, but it is worth being deliberate about.
#
# It runs on its own ports and its own data dir so it never disturbs a node the
# developer already has on 7509.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${FREECHESS_PUBLIC_DIR:-$ROOT/.freenet-public}"

WS_PORT="${FREECHESS_PUBLIC_WS:-7513}"
NET_PORT="${FREECHESS_PUBLIC_NET:-31349}"

dir="$BASE/node"

start() {
  mkdir -p "$dir/config" "$dir/data" "$dir/logs"

  echo "starting public node  ws=$WS_PORT net=$NET_PORT"
  # No --skip-load-from-network: this node WANTS the public gateway list.
  # --disable-auto-update because a bare run has no supervisor to catch the
  # exit-42 update request, and dying mid-publish is worse than staying pinned.
  freenet network \
    --disable-auto-update \
    --network-port "$NET_PORT" \
    --ws-api-port "$WS_PORT" \
    --ws-api-address 127.0.0.1 \
    --config-dir "$dir/config" \
    --data-dir "$dir/data" \
    --log-dir "$dir/logs" \
    --log-level info \
    > "$dir/stdout.log" 2>&1 &
  echo $! > "$dir/pid"
}

wait_ready() {
  local tries=0
  until curl -sf "http://127.0.0.1:$WS_PORT/" > /dev/null 2>&1; do
    if [ -f "$dir/pid" ] && ! kill -0 "$(cat "$dir/pid")" 2>/dev/null; then
      echo "node exited during startup:" >&2
      tail -20 "$dir/stdout.log" >&2
      return 1
    fi
    tries=$((tries + 1))
    if [ "$tries" -gt 120 ]; then
      echo "node did not come up on $WS_PORT" >&2
      tail -20 "$dir/stdout.log" >&2
      return 1
    fi
    sleep 1
  done
  echo "node is up on $WS_PORT"
}

# Joining the real network takes a moment; publishing before there is a single
# connection just times out.
wait_connected() {
  local tries=0
  while true; do
    if grep -qE "connection established|Ring connection established" \
        "$dir"/logs/*.log 2>/dev/null; then
      echo "connected to the public network"
      return 0
    fi
    tries=$((tries + 1))
    if [ "$tries" -gt 120 ]; then
      echo "no peer connection after 120s — check $dir/logs" >&2
      return 1
    fi
    sleep 1
  done
}

case "${1:-up}" in
  up)
    "$0" down > /dev/null 2>&1 || true
    start
    wait_ready
    wait_connected
    echo
    echo "public node: http://127.0.0.1:$WS_PORT/"
    ;;

  down)
    if [ -f "$dir/pid" ]; then
      pid="$(cat "$dir/pid")"
      kill "$pid" 2>/dev/null || true
      for _ in $(seq 30); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
      done
      kill -9 "$pid" 2>/dev/null || true
      rm -f "$dir/pid"
    fi
    echo "public node stopped"
    ;;

  status)
    if curl -sf "http://127.0.0.1:$WS_PORT/" > /dev/null 2>&1; then
      echo "up on $WS_PORT"
      grep -cE "connection established" "$dir"/logs/*.log 2>/dev/null \
        | awk -F: '{s+=$2} END {print "  peer connections seen: " s}'
    else
      echo "down"
    fi
    ;;

  logs)
    tail -f "$dir"/logs/*.log
    ;;

  *)
    echo "usage: $0 {up|down|status|logs}" >&2
    exit 1
    ;;
esac
