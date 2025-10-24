#!/usr/bin/env bash
# End-to-end demo against the real Open5e API:
#   1. look up Fireball and a Goblin, roll adv+3, roll a goblin attack
#   2. restart tavernkeep pointed at a dead upstream
#   3. show cached lookups still working and the breaker opening in /metrics
set -euo pipefail

cd "$(dirname "$0")/.."
PORT="${DEMO_PORT:-18080}"
BASE="http://127.0.0.1:$PORT"
BIN=target/release/tavernkeep
DEAD_UPSTREAM="http://127.0.0.1:9"
[ -x "$BIN" ] || cargo build --release

WORK="$(mktemp -d)"
export TK_CACHE_FILE="$WORK/cache.json"
export TK_BIND="127.0.0.1:$PORT"
# Quiet logs except circuit-breaker transitions.
export TK_LOG="${TK_LOG:-error,tavernkeep::breaker=info}"
TK_PID=""
cleanup() {
  [ -n "$TK_PID" ] && kill "$TK_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

pretty() {
  if command -v jq >/dev/null 2>&1; then jq .; else python3 -m json.tool; fi
}
step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
start() {
  TK_UPSTREAM_URL="$1" "$BIN" serve &
  TK_PID=$!
  for _ in $(seq 1 50); do
    curl -sf "$BASE/healthz" >/dev/null && return 0
    sleep 0.1
  done
  echo "tavernkeep did not start" >&2
  exit 1
}
stop() {
  kill -TERM "$TK_PID"
  wait "$TK_PID" || true
  TK_PID=""
}
status_of() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

step "Start tavernkeep against the real Open5e API"
start "https://api.open5e.com/v1"

step "GET /spells/fireball"
curl -sf "$BASE/spells/fireball" | pretty

step "GET /monsters/goblin (just the headline stats)"
curl -sf "$BASE/monsters/goblin" | python3 -c '
import json, sys
m = json.load(sys.stdin)
print(json.dumps({k: m[k] for k in ("name", "size", "type", "cr", "ac", "hp", "speed")}, indent=2))'

step "Conditional GET with the ETag -> 304"
ETAG="$(curl -sf -D - -o /dev/null "$BASE/spells/fireball" | tr -d '\r' | awk -F': ' 'tolower($1)=="etag"{print $2}')"
echo "ETag: $ETAG -> HTTP $(status_of -H "If-None-Match: $ETAG" "$BASE/spells/fireball")"

step "POST /roll {\"expr\": \"adv+3\"}"
curl -sf -X POST "$BASE/roll" -H 'content-type: application/json' -d '{"expr": "adv+3"}' | pretty

step "GET /monsters/goblin/attack/0 (Scimitar)"
curl -sf "$BASE/monsters/goblin/attack/0" | pretty

step "Simulate an outage: restart pointed at a dead upstream ($DEAD_UPSTREAM)"
stop
echo "snapshot written: $(wc -c < "$TK_CACHE_FILE" | tr -d ' ') bytes"
start "$DEAD_UPSTREAM"

step "Cached lookups still work"
for path in /spells/fireball /monsters/goblin /monsters/goblin/attack/0; do
  echo "GET $path -> HTTP $(status_of "$BASE$path")"
done

step "A never-cached monster fails until the breaker opens, then gets 503 + Retry-After"
for i in 1 2 3 4 5; do
  echo "GET /monsters/lich (#$i) -> HTTP $(status_of "$BASE/monsters/lich")"
done
curl -s -D - -o /dev/null "$BASE/monsters/lich" | tr -d '\r' | grep -i -E '^HTTP|^retry-after'

step "Breaker state in /metrics"
curl -sf "$BASE/metrics" | grep -E '^tk_(breaker_open|upstream_requests_total|cache_events_total)' | sort

step "Fireball is still served while the breaker is open"
echo "GET /spells/fireball -> HTTP $(status_of "$BASE/spells/fireball")"

stop
printf '\n\033[1mDemo complete.\033[0m\n'
