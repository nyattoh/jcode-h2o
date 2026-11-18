"""Parallel worker runner for orchestrating `jcode run` across providers, models, and effort levels.

Designed for Antigravity (as coordinator) to dispatch parallel sub-tasks to
configured `jcode` providers (e.g., `nvidia-nim`, `openai-api`, `grok-build`,
`opencode-go`, `minimax`, `ollama`, `lmstudio`, `gemini-api`) while strictly
blocking `antigravity` provider usage to comply with Google Antigravity Terms of Service.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any

DISALLOWED_PROVIDERS = frozenset({"antigravity"})
VALID_EFFORTS = frozenset({"low", "medium", "high", "xhigh"})
VALID_TOOL_PROFILES = frozenset({"full", "minimal", "lite", "none"})


def resolve_jcode_binary() -> str:
    found = shutil.which("jcode") or shutil.which("jcode.exe")
    if found:
        return found
    local_app = os.environ.get("LOCALAPPDATA", "")
    if local_app:
        candidate = Path(local_app) / "jcode" / "bin" / "jcode.exe"
        if candidate.is_file():
            return str(candidate)
    home_bin = Path.home() / ".local" / "bin" / "jcode"
    if home_bin.is_file():
        return str(home_bin)
    raise FileNotFoundError("jcode binary not found on PATH or standard install locations")


def validate_worker_spec(spec: dict[str, Any], index: int) -> dict[str, Any]:
    if not isinstance(spec, dict):
        raise ValueError(f"worker[{index}] must be a JSON object")

    name = str(spec.get("name") or f"worker-{index + 1}").strip()
    provider = str(spec.get("provider") or "auto").strip()
    if provider.lower() in DISALLOWED_PROVIDERS:
        raise ValueError(
            f"worker[{index}] ({name}): provider '{provider}' is disallowed by Antigravity ToS guard"
        )

    prompt = spec.get("prompt")
    if not isinstance(prompt, str) or not prompt.strip():
        raise ValueError(f"worker[{index}] ({name}): non-empty 'prompt' is required")

    model = spec.get("model")
    if model is not None:
        model = str(model).strip() or None

    effort = spec.get("effort")
    if effort is not None:
        effort = str(effort).strip().lower()
        if effort not in VALID_EFFORTS:
            raise ValueError(
                f"worker[{index}] ({name}): invalid effort '{effort}' (expected one of {sorted(VALID_EFFORTS)})"
            )

    tool_profile = spec.get("tool_profile")
    if tool_profile is not None:
        tool_profile = str(tool_profile).strip().lower()
        if tool_profile not in VALID_TOOL_PROFILES:
            raise ValueError(
                f"worker[{index}] ({name}): invalid tool_profile '{tool_profile}'"
            )

    tools = spec.get("tools")
    if tools is not None:
        if isinstance(tools, list):
            tools = ",".join(str(t).strip() for t in tools if str(t).strip())
        else:
            tools = str(tools).strip()
        tools = tools or None

    cwd = spec.get("cwd")
    if cwd is not None:
        cwd = str(cwd).strip() or None

    timeout_sec = int(spec.get("timeout_sec") or 300)
    if timeout_sec <= 0:
        raise ValueError(f"worker[{index}] ({name}): timeout_sec must be > 0")

    return {
        "name": name,
        "provider": provider,
        "model": model,
        "effort": effort,
        "tool_profile": tool_profile,
        "tools": tools,
        "cwd": cwd,
        "timeout_sec": timeout_sec,
        "prompt": prompt,
    }


def build_jcode_command(jcode_bin: str, worker: dict[str, Any]) -> tuple[list[str], dict[str, str]]:
    cmd = [
        jcode_bin,
        "run",
        "--no-update",
        "--quiet",
        "--json",
        "--provider",
        worker["provider"],
    ]
    if worker["model"]:
        cmd.extend(["--model", worker["model"]])
    if worker["tool_profile"]:
        cmd.extend(["--tool-profile", worker["tool_profile"]])
    if worker["tools"]:
        cmd.extend(["--tools", worker["tools"]])
    if worker["cwd"]:
        cmd.extend(["--cwd", worker["cwd"]])
    cmd.append(worker["prompt"])

    env = os.environ.copy()
    if worker["effort"]:
        env["JCODE_OPENAI_REASONING_EFFORT"] = worker["effort"]
        env["JCODE_ANTHROPIC_REASONING_EFFORT"] = worker["effort"]
    return cmd, env


def run_single_worker(jcode_bin: str, worker: dict[str, Any], dry_run: bool) -> dict[str, Any]:
    cmd, env = build_jcode_command(jcode_bin, worker)
    if dry_run:
        return {
            "name": worker["name"],
            "provider": worker["provider"],
            "model": worker["model"],
            "effort": worker["effort"],
            "status": "dry_run",
            "command": cmd[:-1] + ["<prompt>"],
        }

    started = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            env=env,
            cwd=worker["cwd"] or None,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=worker["timeout_sec"],
        )
        elapsed_ms = int((time.monotonic() - started) * 1000)
        parsed_json: Any = None
        stdout = proc.stdout.strip()
        if stdout:
            try:
                parsed_json = json.loads(stdout)
            except json.JSONDecodeError:
                parsed_json = None

        return {
            "name": worker["name"],
            "provider": worker["provider"],
            "model": worker["model"],
            "effort": worker["effort"],
            "status": "ok" if proc.returncode == 0 else "error",
            "exit_code": proc.returncode,
            "elapsed_ms": elapsed_ms,
            "result": parsed_json if parsed_json is not None else stdout,
            "stderr": proc.stderr.strip() if proc.stderr.strip() else None,
        }
    except subprocess.TimeoutExpired:
        elapsed_ms = int((time.monotonic() - started) * 1000)
        return {
            "name": worker["name"],
            "provider": worker["provider"],
            "model": worker["model"],
            "effort": worker["effort"],
            "status": "timeout",
            "elapsed_ms": elapsed_ms,
            "error": f"Timed out after {worker['timeout_sec']}s",
        }


def run_swarm(specs: list[dict[str, Any]], max_workers: int = 4, dry_run: bool = False) -> dict[str, Any]:
    validated = [validate_worker_spec(s, i) for i, s in enumerate(specs)]
    jcode_bin = "jcode" if dry_run else resolve_jcode_binary()

    results_by_name: dict[str, dict[str, Any]] = {}
    worker_count = max(1, min(max_workers, len(validated)))
    with ThreadPoolExecutor(max_workers=worker_count) as pool:
        future_map = {
            pool.submit(run_single_worker, jcode_bin, w, dry_run): w["name"]
            for w in validated
        }
        for fut in as_completed(future_map):
            res = fut.result()
            results_by_name[res["name"]] = res

    ordered = [results_by_name[w["name"]] for w in validated]
    return {
        "total": len(ordered),
        "succeeded": sum(1 for r in ordered if r["status"] in ("ok", "dry_run")),
        "failed": sum(1 for r in ordered if r["status"] not in ("ok", "dry_run")),
        "workers": ordered,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run parallel `jcode run` workers with per-worker provider, model, and effort."
    )
    parser.add_argument(
        "--spec",
        required=True,
        help="Path to JSON file containing a list of worker specs (or '-' for stdin).",
    )
    parser.add_argument("--max-workers", type=int, default=4, help="Max concurrent workers (default: 4).")
    parser.add_argument("--dry-run", action="store_true", help="Validate specs and print planned commands without executing.")
    args = parser.parse_args()

    raw = sys.stdin.read() if args.spec == "-" else Path(args.spec).read_text(encoding="utf-8")
    data = json.loads(raw)
    if isinstance(data, dict) and "workers" in data:
        specs = data["workers"]
    elif isinstance(data, list):
        specs = data
    else:
        raise ValueError("Spec JSON must be a list of worker objects or {\"workers\": [...]}")

    summary = run_swarm(specs, max_workers=args.max_workers, dry_run=args.dry_run)
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0 if summary["failed"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
