#!/usr/bin/python3
"""Basalgrim executable extension API 1. No third-party Python dependencies."""
import json
import os
from pathlib import Path
import stat
import sys


def read_json(raw):
    fd = os.open(bytes(raw), os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            raise ValueError('Нужен обычный JSON-файл')
        data = source.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError('Пример поддерживает JSON до 1 MiB')
    return json.loads(data)


def main():
    request = json.load(sys.stdin)
    assert request['api'] == 1
    context = request['context']
    if request['hook'] == 'preview':
        text = json.dumps(read_json(context['selected'][0]['path']), ensure_ascii=False, indent=2)
        return {'text': text[:16000]}
    if request['hook'] == 'command':
        if request['command'] == 'validate':
            for entry in context['selected']:
                read_json(entry['path'])
            return {'message': f"JSON корректен: {len(context['selected'])} файл(ов)"}
        raw = context['selected'][0]['path'] if context['selected'] else context['cwd']
        count = 0
        with os.scandir(bytes(raw)) as entries:
            for _ in entries:
                count += 1
                if count >= 10000:
                    return {'message': 'В папке не менее 10000 элементов'}
        return {'message': f'В папке {count} элементов'}
    state = Path(os.environ.get('XDG_STATE_HOME', str(Path.home() / '.local/state')))
    state = state / 'molten-zharr/basalgrim/json-tools'
    state.mkdir(parents=True, exist_ok=True)
    (state / 'last-event.json').write_text(json.dumps(request), encoding='utf-8')
    return {'message': ''}


try:
    print(json.dumps(main(), ensure_ascii=False))
except (OSError, ValueError, AssertionError, KeyError) as error:
    print(str(error), file=sys.stderr)
    sys.exit(1)
