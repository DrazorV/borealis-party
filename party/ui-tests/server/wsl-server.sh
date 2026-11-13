#!/usr/bin/env bash
# Runs a throwaway party server inside WSL for the phone UI tests (server/start.ts starts it with wsl.exe).
# Its config folder, database and data live in a new /tmp folder that is removed when the test run closes stdin.
# Arguments: <server binary> <port> <web dir> <content dir> <admin password> <player password>
#            [<name> <email> <password>]... (more accounts, accounts.ts SEEDED)
set -euo pipefail
bin=$1 port=$2 web=$3 content=$4 admin_pw=$5 player_pw=$6
shift 6

dir=$(mktemp -d /tmp/borealis-ui-tests.XXXXXX)
pid=
cleanup() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  rm -rf "$dir"
}
trap cleanup EXIT

export BOREALIS_CONFIG_DIR="$dir"
export BOREALIS__SERVER__BIND="127.0.0.1:$port"
export BOREALIS__PATHS__WEB="$web"
export BOREALIS__PATHS__CONTENT="$content"
export BOREALIS__DATABASE__TYPE=sqlite
export BOREALIS__LOG__LEVEL="${BOREALIS__LOG__LEVEL:-warn}"

printf '%s\n' "$admin_pw" | "$bin" add-user admin --email admin@phone-rig.test --admin >/dev/null
printf '%s\n' "$player_pw" | "$bin" add-user player --email player@phone-rig.test >/dev/null
while [ "$#" -ge 3 ]; do
  printf '%s\n' "$3" | "$bin" add-user "$1" --email "$2" >/dev/null
  shift 3
done

"$bin" &
pid=$!
echo "phone-rig server pid $pid in $dir"
# Until the test runner closes our stdin (or goes away).
cat >/dev/null || true
