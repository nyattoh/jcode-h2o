#!/usr/bin/env bash
set -euo pipefail

# Direct interactive-shell helper to run official `agy -p` inside `jcode-h2o`
# in compliance with Google Antigravity Terms of Service.

for var in CLAUDECODE CLAUDE_CODE_ENTRYPOINT CODEX_SANDBOX CODEX_THREAD_ID CODEX_CI GEMINI_CLI JCODE_SESSION_ID JCODE_SESSION JCODE_SOCKET JCODE_API_SOCKET OPENCODE; do
  if [[ -n "${!var:-}" ]]; then
    echo "Blocked by scripts/agy-p.sh: external agent environment detected ($var). Do not invoke 'agy -p' via external agents (Antigravity ToS compliance)." >&2
    exit 2
  fi
done

if [[ $# -eq 0 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<'EOF'
Usage: ./scripts/agy-p.sh "<prompt>" [extra agy flags...]

Runs the unmodified official `agy` CLI in headless `-p` mode directly from your shell.
Refuses execution automatically when invoked from external agent environments (jcode / Claude / Codex / Gemini CLI).
EOF
  exit 0
fi

AGY_BIN="$(command -v agy.exe || command -v agy || true)"
if [[ -z "$AGY_BIN" ]]; then
  FALLBACK="${LOCALAPPDATA:-}/agy/bin/agy.exe"
  if [[ -n "$FALLBACK" && -x "$FALLBACK" ]]; then
    AGY_BIN="$FALLBACK"
  else
    echo "Error: agy binary not found on PATH." >&2
    exit 1
  fi
fi

exec "$AGY_BIN" -p "$@"
