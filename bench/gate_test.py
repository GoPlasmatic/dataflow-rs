#!/usr/bin/env python3
"""`gate.py`'s own checks, on stand-in binaries: a smoke number is never a
measurement, and `smoke` holds every example to its contract.

    python3 bench/gate_test.py
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import gate  # noqa: E402

GATE = os.path.join(HERE, "gate.py")


def stand_in(folder, name, line=None, code=0):
    """An executable `name` in `folder` that prints `GATE <line>` when given
    one and exits `code`."""
    path = os.path.join(folder, name)
    body = "#!/bin/sh\n"
    if line is not None:
        body += f"echo 'GATE {json.dumps(line)}'\n"
    body += f"exit {code}\n"
    with open(path, "w") as f:
        f.write(body)
    os.chmod(path, 0o755)


def metric():
    return [{"name": "m", "value": 1.0, "unit": "ns", "better": "lower"}]


def run(*args, env=None):
    env = dict(os.environ if env is None else env)
    return subprocess.run([sys.executable, GATE, *args], capture_output=True, text=True, env=env)


class Smoke(unittest.TestCase):
    def every_example(self, folder, broken=None):
        """Stand-ins for every example: the benchmarks print a smoke line,
        the rest nothing; `broken` prints a full line instead."""
        for name in gate.examples(gate.repo_root()):
            if name in gate.BENCHES:
                line = {"bench": name, "metrics": metric(), "smoke": True}
                if name == broken:
                    del line["smoke"]
                stand_in(folder, name, line)
            else:
                stand_in(folder, name)

    def test_every_example_keeping_its_contract_passes(self):
        with tempfile.TemporaryDirectory() as d:
            self.every_example(d)
            done = run("smoke", "--bins", d)
            self.assertEqual(done.returncode, 0, done.stderr)

    def test_a_benchmark_that_ignores_smoke_mode_fails(self):
        with tempfile.TemporaryDirectory() as d:
            self.every_example(d, broken="fan_out")
            done = run("smoke", "--bins", d)
            self.assertNotEqual(done.returncode, 0)
            self.assertIn("fan_out: its GATE line is not marked smoke", done.stderr)

    def test_a_gate_line_from_an_example_outside_benches_fails(self):
        with tempfile.TemporaryDirectory() as d:
            self.every_example(d)
            stand_in(d, "hello_world", {"bench": "hello_world", "metrics": metric(), "smoke": True})
            done = run("smoke", "--bins", d)
            self.assertNotEqual(done.returncode, 0)
            self.assertIn("hello_world: printed a GATE line but is not in BENCHES", done.stderr)


class Measuring(unittest.TestCase):
    def test_a_smoke_line_is_refused_as_a_measurement(self):
        with self.assertRaises(SystemExit) as refused:
            gate.gate_line("b", 'GATE {"bench": "b", "metrics": [], "smoke": true}')
        self.assertIn("smoke", str(refused.exception))

    def test_compare_refuses_to_start_in_smoke_mode(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            env = dict(os.environ, **{gate.SMOKE: "1"})
            done = run("compare", "--a", a, "--b", b, "--rounds", "1", env=env)
            self.assertNotEqual(done.returncode, 0)
            self.assertIn("refusing to measure", done.stderr)

    def test_a_measured_run_drops_the_smoke_switch(self):
        os.environ[gate.SMOKE] = "1"
        try:
            self.assertNotIn(gate.SMOKE, gate.without_smoke())
        finally:
            del os.environ[gate.SMOKE]

    def test_ratio_and_verdict_statistics(self):
        self.assertAlmostEqual(gate.ratio("lower", 10.0, 12.0), 1.2)
        self.assertAlmostEqual(gate.ratio("higher", 10.0, 5.0), 2.0)
        self.assertEqual(gate.ratio("lower", 0.0, 0.0), 1.0)
        self.assertEqual(gate.half_iqr_pct([1.0]), 0.0)
        self.assertAlmostEqual(gate.half_iqr_pct([1.00, 1.02, 1.04, 1.06]), 1.5)
        self.assertAlmostEqual(gate.spread_pct([90.0, 100.0, 110.0]), 20.0)
        self.assertEqual(gate.verdict(1.05, 3.0), "WORSE")
        self.assertEqual(gate.verdict(0.95, 3.0), "better")
        self.assertEqual(gate.verdict(1.01, 3.0), "not slower")
        self.assertEqual(gate.rule("large_context_handler", "write_only_ns"), "info")
        self.assertEqual(gate.rule("realistic_benchmark", "latency_p50"), "not slower")
        info = gate.machine((1.0, 0.5, 0.25))
        self.assertEqual(info["load_avg_before"], [1.0, 0.5, 0.25])


if __name__ == "__main__":
    unittest.main()
