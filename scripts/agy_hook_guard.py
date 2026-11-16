"""Workspace hook guard blocking external/automated agent calls to `agy -p` / `agy acp`.

Used by `.agents/hooks.json` inside `jcode-h2o` (and compatible with jcode /
Claude Code / Codex / Gemini CLI `PreToolUse` stdin payloads).
"""
from __future__ import annotations

import json
import re
import sys

_AGY_BIN_RE = re.compile(
    r"(?:^|[;&|(\s\"'/\\])agy(?:\.exe|-p(?:\.(?:ps1|cmd|bat|sh))?)?(?=$|[;&|)\s\"'])",
    re.IGNORECASE,
)

_DISALLOWED_MODE_RE = re.compile(
    r"(?:^|\s)(?:-p|--print|--prompt|-i|--prompt-interactive|acp)(?=$|\s|=)"
    r"|(?:^|[;&|(\s\"'/\\])agy-p(?:\.(?:ps1|cmd|bat|sh))?(?=$|[;&|)\s\"'])",
    re.IGNORECASE,
)

_BLOCK_REASON = (
    "Blocked by jcode-h2o agy_hook_guard: Running `agy -p` / `agy acp` / `scripts/agy-p.*` "
    "from an external or automated agent is prohibited to avoid violating Google Antigravity "
    "Terms of Service. Run `./scripts/agy-p.ps1` or `./scripts/agy-p.sh` directly from your "
    "own interactive shell, or use `GEMINI_API_KEY` / Vertex AI for programmatic agent calls."
)


def _extract_command_strings(payload: dict) -> list[str]:
    candidates: list[str] = []

    tool_call = payload.get("toolCall")
    if isinstance(tool_call, dict):
        args = tool_call.get("args")
        if isinstance(args, dict):
            for key in ("CommandLine", "command", "cmd", "script", "args"):
                val = args.get(key)
                if isinstance(val, str) and val.strip():
                    candidates.append(val)

    tool_input = payload.get("tool_input")
    if isinstance(tool_input, dict):
        for key in ("command", "cmd", "CommandLine", "script", "input"):
            val = tool_input.get(key)
            if isinstance(val, str) and val.strip():
                candidates.append(val)
    elif isinstance(tool_input, str) and tool_input.strip():
        candidates.append(tool_input)

    for key in ("command", "cmd", "CommandLine", "script", "input"):
        val = payload.get(key)
        if isinstance(val, str) and val.strip():
            candidates.append(val)

    return candidates


def is_disallowed_agy_command(cmd: str) -> bool:
    if not cmd:
        return False
    return bool(_AGY_BIN_RE.search(cmd) and _DISALLOWED_MODE_RE.search(cmd))


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "exit2"
    try:
        raw = sys.stdin.read()
        payload = json.loads(raw) if raw.strip() else {}
    except Exception:
        return 0

    if not isinstance(payload, dict):
        return 0

    commands = _extract_command_strings(payload)
    if any(is_disallowed_agy_command(c) for c in commands):
        if mode == "antigravity":
            print(json.dumps({"decision": "deny", "reason": _BLOCK_REASON}, ensure_ascii=False))
            return 0
        if mode == "gemini":
            print(json.dumps({"decision": "deny", "reason": _BLOCK_REASON}, ensure_ascii=False))
            print(_BLOCK_REASON, file=sys.stderr)
            return 2
        print(_BLOCK_REASON, file=sys.stderr)
        return 2

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
