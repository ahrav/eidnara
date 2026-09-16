#!/usr/bin/env python3
"""Run one immutable four-position benchmark block; resume verified positions."""

import argparse
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import time


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save(path, value):
    temporary = path.with_suffix('.tmp')
    with temporary.open('w') as stream:
        json.dump(value, stream, indent=2)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def samples(home, expected, count):
    means = {}
    for path in home.glob('**/new/sample.json'):
        data = json.loads(path.read_text())
        name = json.loads((path.parent / 'benchmark.json').read_text())['full_id']
        if name in means or len(data['times']) != count or len(data['iters']) != count:
            raise ValueError('duplicate benchmark or wrong sample count')
        if not all(math.isfinite(x) and x > 0 for x in data['times'] + data['iters']):
            raise ValueError('nonpositive or nonfinite sample')
        means[name] = sum(data['times']) / sum(data['iters'])
    if set(means) != set(expected):
        raise ValueError('benchmark ID set differs from plan')
    return means


def stop(process):
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def capture(path, argv, cwd, env, cpu, timeout, plan_hash):
    path.mkdir(parents=True, exist_ok=False)
    receipt = {'status': 'starting', 'argv': argv, 'cwd': str(cwd),
               'plan_sha256': plan_hash, 'cpu': cpu, 'started_unix': time.time()}
    save(path / 'receipt.json', receipt)
    with os.fdopen(os.open(path / 'environment.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'w') as stream:
        json.dump(env, stream, sort_keys=True)
    process = None
    reader = None
    old_handler = signal.getsignal(signal.SIGTERM)

    def interrupted(signum, frame):
        raise InterruptedError(f'signal {signum}')

    signal.signal(signal.SIGTERM, interrupted)
    start = time.monotonic()
    masks_seen = []
    try:
        with (path / 'stdout').open('w') as out, (path / 'stderr').open('w') as err, (path / 'affinity.jsonl').open('w') as affinity, (path / 'progress.jsonl').open('w') as progress:
            process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err,
                                       start_new_session=True)
            reader = (path / 'stderr').open()
            receipt.update(status='running', pid=process.pid, clock_ticks=os.sysconf('SC_CLK_TCK'))
            save(path / 'receipt.json', receipt)
            while process.poll() is None:
                elapsed = time.monotonic() - start
                if elapsed > timeout:
                    raise TimeoutError(f'attempt exceeded {timeout}s')
                if elapsed > 0.1:
                    masks = {}
                    for task in Path(f'/proc/{process.pid}/task').glob('*'):
                        try:
                            masks[task.name] = sorted(os.sched_getaffinity(int(task.name)))
                        except ProcessLookupError:
                            pass
                    if masks:
                        sample = {'seconds': elapsed, 'tasks': masks}
                        try:
                            stat = Path(f'/proc/{process.pid}/stat').read_text().rsplit(')', 1)[1].split()
                            sample.update(cpu_ticks=int(stat[11]) + int(stat[12]),
                                          minor_faults=int(stat[7]), major_faults=int(stat[9]))
                        except FileNotFoundError:
                            pass
                        affinity.write(json.dumps(sample) + '\n')
                        affinity.flush()
                        masks_seen.extend(masks.values())
                chunk = reader.read()
                if chunk:
                    progress.write(json.dumps({'seconds': elapsed, 'stderr': chunk}) + '\n')
                    progress.flush()
                time.sleep(0.05)
            reader.close()
            receipt['returncode'] = process.returncode
            if process.returncode != 0:
                raise RuntimeError(f'benchmark exited {process.returncode}')
            if not masks_seen or any(mask != [cpu] for mask in masks_seen):
                raise RuntimeError('missing or incorrect observed CPU affinity')
        receipt['status'] = 'captured'
    except BaseException as error:
        receipt.update(status='failed', error=repr(error))
        raise
    finally:
        if reader is not None:
            reader.close()
        if process is not None:
            stop(process)
            receipt['returncode'] = process.returncode
        receipt['elapsed_seconds'] = time.monotonic() - start
        save(path / 'receipt.json', receipt)
        signal.signal(signal.SIGTERM, old_handler)
    return receipt


def cpuset():
    membership = Path('/proc/self/cgroup').read_text()
    group = next(line[3:] for line in membership.splitlines() if line.startswith('0::'))
    path = Path('/sys/fs/cgroup') / group.lstrip('/') / 'cpuset.cpus.effective'
    while not path.is_file():
        if path.parent == Path('/sys/fs/cgroup'):
            raise RuntimeError('no effective cpuset controller')
        path = path.parent.parent / path.name
    allowed = set()
    for item in path.read_text().strip().split(','):
        ends = [int(value) for value in item.split('-')]
        allowed.update(range(ends[0], ends[-1] + 1))
    return membership, str(path), allowed


def run_block(plan_path, block):
    plan_path = plan_path.resolve()
    root = plan_path.parent
    plan = json.loads(plan_path.read_text())
    plan_hash = sha256(plan_path)
    if sha256(__file__) != plan['collector_sha256']:
        raise ValueError('collector changed')
    membership, controller, allowed = cpuset()
    if plan['cpu'] not in allowed or plan['cpu'] not in os.sched_getaffinity(0):
        raise ValueError('planned CPU not permitted')
    save(root / 'cpuset.json', {'membership': membership, 'controller': controller,
                                'allowed': sorted(allowed)})
    for position, label in enumerate(plan['blocks'][block]):
        artifact = plan['artifacts'][label]
        binary = Path(artifact['path'])
        if sha256(binary) != artifact['sha256']:
            raise ValueError('artifact changed')
        path = root / f'block-{block:02}' / f'{position}-{label}'
        if path.exists():
            receipt = json.loads((path / 'receipt.json').read_text())
            if receipt['status'] != 'valid' or receipt['plan_sha256'] != plan_hash:
                raise RuntimeError(f'unresolved attempt {path}; no automatic replacement')
            if any(sha256(path / name) != checksum for name, checksum in receipt['evidence'].items()):
                raise ValueError('retained evidence changed')
            print(f'verified existing {path.name}', flush=True)
            continue
        env = os.environ.copy()
        env.update(CRITERION_HOME=str(path / 'criterion'), RAYON_NUM_THREADS='1')
        argv = ['taskset', '-c', str(plan['cpu']), str(binary), *plan['argv']]
        receipt = capture(path, argv, plan['cwd'], env, plan['cpu'], plan['timeout_seconds'], plan_hash)
        try:
            if sha256(binary) != artifact['sha256'] or sha256(plan_path) != plan_hash:
                raise ValueError('artifact or plan changed during attempt')
            stderr = (path / 'stderr').read_text()
            if stderr.count('claim_read fixture=') != len(plan['fixture_lines']) or any(line not in stderr for line in plan['fixture_lines']):
                raise ValueError('missing or extra fixture canary')
            receipt['means_ns'] = samples(path / 'criterion', plan['benchmark_ids'], plan['sample_count'])
            receipt['artifact_sha256'] = artifact['sha256']
            receipt['evidence'] = {str(file.relative_to(path)): sha256(file) for file in path.rglob('*')
                                   if file.is_file() and file.name != 'receipt.json'}
            receipt['status'] = 'valid'
        except BaseException as error:
            receipt.update(status='failed', error=repr(error))
            save(path / 'receipt.json', receipt)
            raise
        save(path / 'receipt.json', receipt)
        print(f'completed block={block} position={position} label={label}', flush=True)
    print(f'block {block} complete', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('plan', type=Path)
    parser.add_argument('block', type=int)
    args = parser.parse_args()
    with (args.plan.parent / 'collector.lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        run_block(args.plan, args.block)


if __name__ == '__main__':
    main()
