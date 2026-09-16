import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

import bench_claim_read as runner


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.cpu = min(os.sched_getaffinity(0))

    def tearDown(self):
        self.directory.cleanup()

    def command(self, code):
        return ['taskset', '-c', str(self.cpu), sys.executable, '-c', code]

    def test_capture_and_timeout_are_retained(self):
        path = self.root / 'success'
        result = runner.capture(path, self.command('import time; time.sleep(.3); print("done")'),
                                self.root, os.environ.copy(), self.cpu, 5, 'plan')
        self.assertEqual(result['status'], 'captured')
        self.assertEqual((path / 'stdout').read_text().strip(), 'done')
        self.assertTrue((path / 'affinity.jsonl').read_text())
        with self.assertRaises(FileExistsError):
            runner.capture(path, [], self.root, {}, self.cpu, 5, 'plan')
        failed = self.root / 'timeout'
        with self.assertRaises(TimeoutError):
            runner.capture(failed, self.command('import time; time.sleep(30)'),
                           self.root, os.environ.copy(), self.cpu, .3, 'plan')
        receipt = json.loads((failed / 'receipt.json').read_text())
        self.assertEqual(receipt['status'], 'failed')
        self.assertNotEqual(receipt['returncode'], 0)
        with self.assertRaises(ProcessLookupError):
            os.kill(receipt['pid'], 0)

    def test_sigterm_reaps_child_and_preserves_receipt(self):
        path = self.root / 'interrupt'
        command = self.command('import time; time.sleep(30)')
        code = (
            'import os, pathlib, bench_claim_read as r; '
            f'r.capture(pathlib.Path({str(path)!r}), {command!r}, '
            f'{str(self.root)!r}, os.environ.copy(), {self.cpu}, 40, "plan")'
        )
        process = subprocess.Popen([sys.executable, '-c', code], cwd=Path(__file__).parent,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                file = path / 'receipt.json'
                if file.exists() and json.loads(file.read_text())['status'] == 'running':
                    break
                time.sleep(.01)
            else:
                self.fail('collector never started')
            process.send_signal(signal.SIGTERM)
            process.wait(timeout=10)
            receipt = json.loads((path / 'receipt.json').read_text())
            self.assertEqual(receipt['status'], 'failed')
            self.assertIn('InterruptedError', receipt['error'])
            with self.assertRaises(ProcessLookupError):
                os.kill(receipt['pid'], 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_resume_verifies_evidence_without_rerunning(self):
        code = (
            'import os,pathlib,json,time; '
            'p=pathlib.Path(os.environ["CRITERION_HOME"])/"case/new"; '
            'p.mkdir(parents=True); '
            '(p/"sample.json").write_text(json.dumps({"times":[1,2],"iters":[1,1]})); '
            '(p/"benchmark.json").write_text(json.dumps({"full_id":"case"})); '
            'time.sleep(.3)'
        )
        artifact = {'path': sys.executable, 'sha256': runner.sha256(sys.executable)}
        plan = dict(cpu=self.cpu, blocks=['ABBA'], artifacts={'A': artifact, 'B': artifact},
                    argv=['-c', code], cwd=str(self.root), timeout_seconds=5,
                    fixture_lines=[], benchmark_ids=['case'], sample_count=2,
                    collector_sha256=runner.sha256(runner.__file__))
        file = self.root / 'plan.json'
        file.write_text(json.dumps(plan))
        runner.run_block(file, 0)
        receipts = sorted(self.root.glob('block-*/*/receipt.json'))
        before = [p.read_bytes() for p in receipts]
        runner.run_block(file, 0)
        self.assertEqual(before, [p.read_bytes() for p in receipts])
        plan['timeout_seconds'] = 6
        file.write_text(json.dumps(plan))
        with self.assertRaises(RuntimeError):
            runner.run_block(file, 0)
        plan['timeout_seconds'] = 5
        file.write_text(json.dumps(plan))
        sample = next(self.root.glob('block-*/*/criterion/**/sample.json'))
        sample.write_text('{}')
        with self.assertRaises(ValueError):
            runner.run_block(file, 0)


if __name__ == '__main__':
    unittest.main()
