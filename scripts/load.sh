#!/usr/bin/env bash
# Warm-cache load test. Serves the recorded fixtures from a local static
# server (no network), warms tavernkeep, then measures p50/p95/p99 and rps
# with `oha` if installed, otherwise target/release/loadgen.
set -euo pipefail

cd "$(dirname "$0")/.."
DURATION="${DURATION:-15}"
CONCURRENCY="${CONCURRENCY:-64}"
UP_PORT="${UP_PORT:-18900}"
TK_PORT="${TK_PORT:-18901}"
BIN=target/release/tavernkeep
[ -x "$BIN" ] || cargo build --release

WORK="$(mktemp -d)"
cleanup() {
  [ -n "${TK_PID:-}" ] && kill "$TK_PID" 2>/dev/null || true
  [ -n "${UP_PID:-}" ] && kill "$UP_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# Lay fixtures out as <collection>/<slug>/index.html so a static server
# answers /spells/fireball/ like Open5e does.
for kind in spells monsters; do
  for f in tests/fixtures/$kind/*.json; do
    slug="$(basename "$f" .json)"
    mkdir -p "$WORK/$kind/$slug"
    cp "$f" "$WORK/$kind/$slug/index.html"
  done
done
python3 -m http.server "$UP_PORT" --bind 127.0.0.1 --directory "$WORK" >/dev/null 2>&1 &
UP_PID=$!

TK_BIND="127.0.0.1:$TK_PORT" TK_UPSTREAM_URL="http://127.0.0.1:$UP_PORT" TK_LOG=warn "$BIN" serve &
TK_PID=$!
for _ in $(seq 1 50); do
  curl -sf "http://127.0.0.1:$TK_PORT/healthz" >/dev/null && break
  sleep 0.1
done
for _ in $(seq 1 50); do
  curl -sf "http://127.0.0.1:$UP_PORT/spells/fireball/" >/dev/null && break
  sleep 0.1
done

"$BIN" warm --file party.example.txt --url "http://127.0.0.1:$TK_PORT" >/dev/null
URL="http://127.0.0.1:$TK_PORT/monsters/adult-red-dragon"

echo "== machine"
uname -srm
if [ "$(uname)" = "Darwin" ]; then
  echo "cpu: $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores, $(( $(sysctl -n hw.memsize) / 1073741824 )) GiB RAM"
else
  echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs), $(nproc) cores"
fi
echo "== load: GET $URL, warm cache, ${CONCURRENCY} connections, ${DURATION}s"

if command -v oha >/dev/null 2>&1; then
  echo "tool: $(oha --version)"
  oha --no-tui -z "${DURATION}s" -c "$CONCURRENCY" "$URL"
else
  [ -x target/release/loadgen ] || cargo build --release --bin loadgen
  echo "tool: target/release/loadgen"
  target/release/loadgen --url "$URL" --duration "$DURATION" --concurrency "$CONCURRENCY"
fi
