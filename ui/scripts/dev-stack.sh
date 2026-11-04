#!/usr/bin/env bash
# A local Chat to develop the interface against: Postgres in a container, the Rust
# API, and `next dev` in front of it. Open http://127.0.0.1:$UI_PORT and sign in as
# admin with $ADMIN_PASSWORD. No gateway, so models and replies are unavailable.
#
#   scripts/dev-stack.sh up   NAME UI_PORT API_PORT DB_PORT
#   scripts/dev-stack.sh down NAME
#
# OBLETH_CHAT_BIN may point at a built obleth-chat binary; otherwise it is built.
set -euo pipefail
cmd=${1:?up or down}; name=${2:?name}
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/.." && pwd)
state=${TMPDIR:-/tmp}/chat-dev-stack-$name
mkdir -p "$state"
if [ "$cmd" = down ]; then
  # Each server leads its own process group (setsid), so this stops next dev's children too.
  for pid in api ui; do [ -f "$state/$pid.pid" ] && kill -- "-$(cat "$state/$pid.pid")" 2>/dev/null || kill "$(cat "$state/$pid.pid")" 2>/dev/null || true; done
  # next dev runs its server as a detached child, outside that group: stop whatever
  # from this checkout still listens on the stack's port.
  if [ -f "$state/ui.port" ]; then
    for pid in $(ss -ltnpH "sport = :$(cat "$state/ui.port")" 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
      case "$(readlink "/proc/$pid/cwd" 2>/dev/null)" in "$here"*) kill "$pid" 2>/dev/null || true ;; esac
    done
  fi
  podman rm -f "chat-dev-$name" >/dev/null 2>&1 || true
  rm -rf "$state"
  echo "stopped $name"
  exit 0
fi
ui_port=${3:?ui port}; api_port=${4:?api port}; db_port=${5:?db port}
password=${ADMIN_PASSWORD:-local-test-password-1}
echo "$ui_port" >"$state/ui.port"
podman run -d --rm --name "chat-dev-$name" -e POSTGRES_USER=obleth -e POSTGRES_PASSWORD=obleth \
  -p "127.0.0.1:$db_port:5432" docker.io/pgvector/pgvector:0.8.7-pg17-trixie </dev/null >/dev/null 2>&1
for _ in $(seq 30); do podman exec "chat-dev-$name" pg_isready -U obleth >/dev/null 2>&1 && break; sleep 1; done
bin=${OBLETH_CHAT_BIN:-}
if [ -z "$bin" ]; then
  (cd "$root" && cargo build --locked --quiet)
  bin="$root/target/debug/obleth-chat"
fi
OBLETH_CHAT_DATABASE_URL="postgres://obleth:obleth@127.0.0.1:$db_port/postgres" \
OBLETH_CHAT_LISTEN="127.0.0.1:$api_port" \
OBLETH_CHAT_ORIGIN="http://127.0.0.1:$ui_port" \
OBLETH_CHAT_BOOTSTRAP_USERNAME=admin OBLETH_CHAT_BOOTSTRAP_PASSWORD="$password" \
OBLETH_CHAT_BRANDING_DIR="$root/branding" \
  setsid nohup "$bin" </dev/null >"$state/api.log" 2>&1 &
echo $! >"$state/api.pid"
(cd "$here" && OBLETH_CHAT_API_URL="http://127.0.0.1:$api_port" NEXT_TELEMETRY_DISABLED=1 setsid nohup npx next dev --port "$ui_port" --hostname 127.0.0.1 </dev/null >"$state/ui.log" 2>&1 & echo $! >"$state/ui.pid")
for _ in $(seq 60); do curl -sf "http://127.0.0.1:$ui_port/activate/check" >/dev/null 2>&1 && break; sleep 1; done
echo "up: http://127.0.0.1:$ui_port (admin / \$ADMIN_PASSWORD); logs in $state"
