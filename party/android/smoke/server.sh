#!/usr/bin/env bash
# A throwaway party server in WSL for the Android smoke run (../smoke-open-with.ps1). Never the production server:
# a new /tmp folder holds its config and SQLite database, and `stop` removes it.
#   server.sh start <server binary> <port> <repo root>   bind 0.0.0.0:<port> (the emulator reaches it at 10.0.2.2);
#                                                         the repo's content/ and ui/party/dist
#   server.sh stop <port>
# The password comes on stdin for `start`, so it is never on a command line.
set -euo pipefail
state() { echo "/tmp/bp-smoke-$1.state"; }

case "${1:-}" in
  start)
    bin=$2 port=$3 repo=$4
    IFS= read -r password
    [ -x "$bin" ] || { echo "no server binary at $bin" >&2; exit 1; }
    if [ -f "$(state "$port")" ]; then "$0" stop "$port"; fi
    dir=$(mktemp -d /tmp/bp-smoke.XXXXXX)
    export BOREALIS_CONFIG_DIR="$dir"
    export BOREALIS__SERVER__BIND="0.0.0.0:$port"
    export BOREALIS__DATABASE__TYPE=sqlite
    export BOREALIS__PATHS__CONTENT="$repo/content"
    export BOREALIS__PATHS__WEB="$repo/ui/party/dist"
    export BOREALIS__LOG__LEVEL="${BOREALIS__LOG__LEVEL:-warn}"
    printf '%s\n' "$password" | "$bin" add-user smoke --email smoke@smoke.test >/dev/null
    nohup "$bin" >"$dir/server.log" 2>&1 &
    echo "$! $dir" >"$(state "$port")"
    for _ in $(seq 1 60); do
      if curl -fsS "http://127.0.0.1:$port/api/server" >/dev/null 2>&1; then echo "server up on $port ($dir)"; exit 0; fi
      sleep 0.5
    done
    echo "the server did not start; its log:" >&2
    cat "$dir/server.log" >&2
    exit 1
    ;;
  stop)
    port=$2
    f=$(state "$port")
    [ -f "$f" ] || exit 0
    read -r pid dir <"$f"
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
    case "$dir" in /tmp/bp-smoke.*) rm -rf "$dir" ;; esac
    rm -f "$f"
    echo "server on $port stopped and removed"
    ;;
  *) echo "usage: server.sh start <bin> <port> | stop <port>" >&2; exit 2 ;;
esac
