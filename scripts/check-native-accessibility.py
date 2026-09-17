#!/usr/bin/env python3
"""Read-only native navigation smoke test; requires a visible, onboarded app.

Run outside an execution sandbox. No coordinate/OCR/image locators are accepted.
The one process owns each Docwalk attach/action/close lifetime. Screenshots and
raw trees may contain private app data; output belongs under ignored .docwalk/.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import time


def nodes(node):
    yield node
    for child in node.get('children', []):
        yield from nodes(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--docwalk', type=Path, default=Path('../docwalk/target/debug/docwalk'))
    parser.add_argument('--out', type=Path, default=Path('.docwalk/native-a11y') / time.strftime('%Y%m%d-%H%M%S'))
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)

    def call(label, *command):
        result = subprocess.run([str(args.docwalk), *command], capture_output=True, text=True, timeout=60)
        (args.out / f'{label}.json').write_text(result.stdout)
        (args.out / f'{label}.stderr').write_text(result.stderr)
        if result.returncode:
            raise RuntimeError(f'{label}: {result.stdout} {result.stderr}')
        response = json.loads(result.stdout)
        assert response.get('ok', True), (label, response)
        return response

    expected = {
        'Transport': r'^(Drop anything\.|Going\.|Landed\.)$',
        'Pads': r'^Your pads\.$',
        'Log': r'^(Nothing yet\.|\d+ transports?\.|\d+ in flight\.)$',
        'Config': r'^This pad\.$',
    }
    for run in (1, 2):
        # Docwalk's --pid path currently prefers any frontmost owned window,
        # even a titlebar tooltip, ahead of --title-regex. Select the main
        # window by title and verify its actual owner before sending input.
        attached = call(f'{run}-attach', 'attach', '--title-regex', '^Fileporter$')
        session = attached['session']
        try:
            assert attached['window_title'] == 'Fileporter', attached
            log = Path(attached['session_dir']) / 'daemon.log'
            owner_evidence = log.read_text()
            (args.out / f'{run}-owner.log').write_text(owner_evidence)
            assert f'(pid {args.pid})' in owner_evidence, 'Captured owner does not match --pid; no input sent'
            # Begin away from Transport so its first activation produces a
            # screen change. Clicking an already-selected page can legitimately
            # produce no new capture frame, which is not a navigation failure.
            prepared = call(f'{run}-prepare', 'click', '--session', session, '--locator',
                            json.dumps({'ax': {'role': 'button', 'name': 'Config'}}))
            assert prepared['resolved']['tier'] == 1 and not prepared['resolved']['drift'], prepared
            for step, name in enumerate(['Transport', 'Pads', 'Log', 'Config', 'Transport']):
                label = f'{run}-{step}-{name}'
                found = call(label + '-find', 'find', '--session', session, '--role', 'button', '--name', name)
                assert found['total'] == 1, (label, found)
                locator = json.dumps({'ax': {'role': 'button', 'name': name}})
                clicked = call(label + '-click', 'click', '--session', session, '--locator', locator)
                assert clicked['resolved']['tier'] == 1 and not clicked['resolved']['drift'], (label, clicked)
                shot = call(label + '-shot', 'shot', '--session', session, '--tree')
                assert shot['settled'] and shot['settle'] in ('settled', 'already_still'), (label, shot['settle'])
                assert shot['age_ms'] < 10000, (label, 'stale capture', shot['age_ms'])
                assert shot['tree']['role'] == 'window', (label, shot.get('tree_note'))
                assert not shot.get('tree_truncated', False), label
                tree = list(nodes(shot['tree']))
                for navigation in expected:
                    matches = [n for n in tree if n.get('role') == 'button' and n.get('name') == navigation]
                    assert len(matches) == 1 and matches[0]['enabled'], (label, navigation)
                    assert matches[0]['rect'][2] > 0 and matches[0]['rect'][3] > 0, (label, navigation)
                headings = [n.get('name', '') for n in tree if n.get('role') == 'heading']
                assert any(re.fullmatch(expected[name], h) for h in headings), (label, headings)
                if name == 'Pads':
                    assert any(n.get('role') == 'button' and n.get('name') == 'Add' for n in tree), label
                    assert any(n.get('role') == 'textfield' and n.get('name') == 'Add a pad by address' for n in tree), label
                if name == 'Config':
                    assert any(n.get('role') == 'button' and n.get('name') == 'Choose' for n in tree), label
                    for action in ('Apply', 'Discard'):
                        assert any(n.get('name') == action and n.get('role') == 'button' and not n['enabled'] for n in tree), (label, action)
                print(f'{label}: AX tier 1; {headings}; screenshot {shot["image"]}', flush=True)
        finally:
            call(f'{run}-close', 'close', '--session', session)
    print(f'PASS: two native navigation sequences; evidence in {args.out}', flush=True)


if __name__ == '__main__':
    main()
