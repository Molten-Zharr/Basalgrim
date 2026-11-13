"""Ordinary GUI commands reach pkexec; no_new_privs prevents host authorization."""
import ctypes
from contextlib import closing
import json
import os
import sqlite3
import time


def sandbox():
    # Applied only in the isolated test child, before exec of the real binary.
    # pkexec must stop before any polkit/root action. No production injection.
    if ctypes.CDLL(None, use_errno=True).prctl(38, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'PR_SET_NO_NEW_PRIVS')


def last_operation(root):
    path=root.parent/'state/molten-zharr/basalgrim/operations.sqlite3'
    if not path.exists():return None
    with closing(sqlite3.connect(path)) as db:
        row=db.execute('SELECT state,summary FROM operations ORDER BY id DESC LIMIT 1').fetchone()
    return (row[0],json.loads(row[1])) if row and row[1] else None


def operation_id(root):
    with closing(sqlite3.connect(root.parent/'state/molten-zharr/basalgrim/operations.sqlite3')) as db:
        return db.execute('SELECT max(id) FROM operations').fetchone()[0]


def wait(predicate,label):
    until=time.monotonic()+8
    while not predicate():
        assert time.monotonic()<until,label
        time.sleep(.05)


def exercise(root,key,text,click,screenshot,compact=False,copy_text=None):
    root.chmod(0o700)
    suffix='compact' if compact else 'normal'
    name='Проверка-'+suffix+'.txt'
    key('n','ctrl');time.sleep(.25);text(name);time.sleep(.2)
    if copy_text:
        key('a','ctrl');key('c','ctrl');time.sleep(.3)
        assert copy_text()==name,'native editor lost Unicode text'
        key('End')
    screenshot(suffix+'-ordinary-create')
    # Ordinary 68x10 name dialog, centered at 170x53 or 50x22.
    click(5 if compact else 55,14 if compact else 29)
    wait(lambda:(root/name).exists(),'ordinary create did not complete directly')
    wait(lambda:last_operation(root) and last_operation(root)[0]=='done','normal operation history')
    root.chmod(0o500)
    try:
        key('n','ctrl');time.sleep(.25);text('protected-'+suffix+'.txt');time.sleep(.2)
        click(5 if compact else 55,14 if compact else 29)
        wait(lambda:last_operation(root) and last_operation(root)[0]=='failed','protected operation did not reach pkexec')
        state,result=last_operation(root)
        assert 'pkexec' in str(result),result
        assert not (root/('protected-'+suffix+'.txt')).exists()
        screenshot(suffix+'-authorization-unavailable')
    finally:
        root.chmod(0o700)
    # Escape closes an ordinary error dialog, if shown, without another wizard.
    key('Escape');time.sleep(.2)
    target=root.parent/('target-'+suffix)
    target.write_text('target must remain')
    link=root/('syslog-'+suffix+'.service');link.symlink_to(target)
    key('F5');time.sleep(.3);key('slash');text(link.name);key('Return');time.sleep(.3)
    previous=operation_id(root)
    root.chmod(0o500)
    try:
        key('F8');time.sleep(.2);screenshot(suffix+'-trash-confirm')
        click(5 if compact else 55,15 if compact else 30)
        wait(lambda:operation_id(root)>previous and last_operation(root) and last_operation(root)[0]=='failed','protected Trash did not request authorization')
        assert 'pkexec' in str(last_operation(root)),last_operation(root)
        assert link.is_symlink() and target.read_text()=='target must remain'
        screenshot(suffix+'-trash-auth-unavailable')
    finally:root.chmod(0o700)
    key('Escape');time.sleep(.15)
    key('slash');text(link.name);key('Return');time.sleep(.2)
    key('F8');time.sleep(.2);click(5 if compact else 55,15 if compact else 30)
    wait(lambda:not link.is_symlink(),'ordinary Trash failed')
    try:
        wait(lambda:last_operation(root) and last_operation(root)[0]=='done','ordinary Trash history')
    except AssertionError:
        screenshot(suffix+'-trash-history-failure')
        raise AssertionError(f'ordinary Trash history: {last_operation(root)}') from None
    trashed=list((root.parent/'data/Trash/files').iterdir())
    assert any(p.is_symlink() and os.readlink(p)==str(target) for p in trashed)
    assert target.read_text()=='target must remain'
    key('Escape');time.sleep(.15)
    print('Automatic authorization native: protected Trash symlink reaches pkexec; denied authorization preserves source/target; ordinary Trash reaches caller catalog; create controls remain valid',flush=True)
