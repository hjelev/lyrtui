#!/usr/bin/env bash
# Driver for lyrtui — wraps the TUI in a detached tmux session for agent use.
# Usage: driver.sh <command> [args]
#   launch [WxH]   — build (if needed) and start lyrtui in tmux (default 120x40)
#   ss             — capture current screen to stdout
#   send <keys>    — send keys (tmux send-keys syntax, e.g. "j" "Enter" "Escape")
#   quit           — send 'q' and kill the session
#   status         — print "running" or "stopped"

SESSION="lyrtui-driver"
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"

case "${1:-}" in
  launch)
    DIMS="${2:-120x40}"
    W="${DIMS%%x*}"
    H="${DIMS##*x}"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    tmux new-session -d -s "$SESSION" -x "$W" -y "$H"
    tmux send-keys -t "$SESSION" "cd '$REPO' && cargo run 2>/tmp/lyrtui-err.txt" Enter
    # Wait up to 15s for the app to appear (Navigation box or art mode)
    for i in $(seq 1 30); do
      sleep 0.5
      SCREEN=$(tmux capture-pane -t "$SESSION" -p 2>/dev/null)
      if echo "$SCREEN" | grep -q "Navigation"; then
        echo "ready"
        exit 0
      fi
      # App may start in art mode (persisted toggle state); exit it automatically
      if echo "$SCREEN" | grep -q "exit art"; then
        tmux send-keys -t "$SESSION" "\`" ""
        sleep 0.5
        if tmux capture-pane -t "$SESSION" -p 2>/dev/null | grep -q "Navigation"; then
          echo "ready"
          exit 0
        fi
      fi
    done
    echo "timeout — app did not start within 15s" >&2
    cat /tmp/lyrtui-err.txt >&2
    exit 1
    ;;

  ss)
    tmux capture-pane -t "$SESSION" -p
    ;;

  send)
    shift
    for key in "$@"; do
      tmux send-keys -t "$SESSION" "$key" ""
      sleep 0.1
    done
    sleep 0.3
    tmux capture-pane -t "$SESSION" -p
    ;;

  quit)
    tmux send-keys -t "$SESSION" "q" "" 2>/dev/null || true
    sleep 0.3
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    echo "stopped"
    ;;

  status)
    if tmux has-session -t "$SESSION" 2>/dev/null; then
      echo "running"
    else
      echo "stopped"
    fi
    ;;

  *)
    echo "Usage: $0 launch|ss|send|quit|status" >&2
    exit 1
    ;;
esac
