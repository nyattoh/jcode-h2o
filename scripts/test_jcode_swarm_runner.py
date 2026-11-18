"""Unit tests for scripts/jcode_swarm_runner.py (no network / no paid API calls)."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest

RUNNER_PATH = Path(__file__).resolve().parent / "jcode_swarm_runner.py"
spec = importlib.util.spec_from_file_location("jcode_swarm_runner", RUNNER_PATH)
assert spec and spec.loader
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class JcodeSwarmRunnerTests(unittest.TestCase):
    def test_blocks_antigravity_provider(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            runner.validate_worker_spec(
                {"name": "bad", "provider": "antigravity", "prompt": "hi"},
                0,
            )
        self.assertIn("disallowed", str(ctx.exception))

    def test_validates_effort_and_builds_command(self) -> None:
        w = runner.validate_worker_spec(
            {
                "name": "gemma-worker",
                "provider": "nvidia-nim",
                "model": "gemma-4-31b",
                "effort": "high",
                "tool_profile": "minimal",
                "prompt": "Summarize README.md",
            },
            0,
        )
        cmd, env = runner.build_jcode_command("jcode", w)
        self.assertIn("--provider", cmd)
        self.assertIn("nvidia-nim", cmd)
        self.assertIn("gemma-4-31b", cmd)
        self.assertEqual(env["JCODE_OPENAI_REASONING_EFFORT"], "high")
        self.assertEqual(env["JCODE_ANTHROPIC_REASONING_EFFORT"], "high")

    def test_dry_run_preserves_order(self) -> None:
        summary = runner.run_swarm(
            [
                {"name": "w1", "provider": "nvidia-nim", "model": "gemma-4-31b", "prompt": "task 1"},
                {"name": "w2", "provider": "openai-api", "effort": "medium", "prompt": "task 2"},
                {"name": "w3", "provider": "ollama", "model": "gemma3:27b", "prompt": "task 3"},
            ],
            max_workers=3,
            dry_run=True,
        )
        self.assertEqual(summary["total"], 3)
        self.assertEqual(summary["succeeded"], 3)
        self.assertEqual(summary["failed"], 0)
        self.assertEqual([w["name"] for w in summary["workers"]], ["w1", "w2", "w3"])


if __name__ == "__main__":
    unittest.main()
