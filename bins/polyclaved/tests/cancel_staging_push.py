"""Offline macOS daemon probe with a socket barrier in place of Git push."""

import json
import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import urllib.request
from contextlib import closing
daemon = pathlib.Path(sys.argv[1]).resolve()
root = pathlib.Path(sys.argv[2])
with tempfile.TemporaryDirectory(dir=root) as raw:
    tmp = pathlib.Path(raw)
    (tmp/'bin').mkdir()
    (tmp/'home').mkdir()
    (tmp/'user-codex').mkdir()
    (tmp/'user-codex'/'auth.json').write_text('{}')
    repo = tmp/'repo'
    repo.mkdir()
    git = shutil.which('git')
    def run_git(*args):
        return subprocess.check_output([git, '-C', str(repo), *args], text=True).strip()
    run_git('init', '-b', 'main', '--template=')
    run_git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty','-m','base')
    worker = tmp/'bin'/'codex'
    worker.write_text(f'#!{sys.executable}\n'+'''
import json, os, pathlib, sys, urllib.request
prompt = sys.stdin.read()
def get(prefix):
    return next(line[len(prefix):] for line in prompt.splitlines() if line.startswith(prefix))
url = next(json.loads(x.split('=', 1)[1]) for x in sys.argv if x.startswith('mcp_servers.host.url='))
def call(name, args):
    data = json.dumps(dict(jsonrpc='2.0',id=1,method='tools/call',params=dict(name=name,arguments=args))).encode()
    req = urllib.request.Request(url,data,{'Content-Type':'application/json','Authorization':'Bearer '+os.environ['POLYCLAVE_MCP_TOKEN']})
    with urllib.request.urlopen(req, timeout=20) as resp:
        result = json.load(resp)['result']
    assert result['isError'] is False, result
    return result
pathlib.Path('allowed.txt').write_text('scoped fixture')
result = call('commit_and_stage',dict(message='fixture',paths=['allowed.txt']))
sha = result['structuredContent']['sha']
call('submit_result',dict(run_id=get('Run: '),branch=get('Attempt branch: '),sha=sha))
''')
    worker.chmod(0o700)
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen()
    listener.settimeout(12)
    hostgit = tmp/'bin'/'git'
    hostgit.write_text(f'#!{sys.executable}\n'+'''
import os, socket, sys, pathlib
flag = pathlib.Path(os.environ['REVIEW_PUSH_FLAG'])
if 'push' in sys.argv and flag.exists():
    flag.unlink()
    with socket.create_connection(('127.0.0.1', int(os.environ['REVIEW_BARRIER'])), timeout=10) as s:
        s.sendall(b'push started')
        assert s.recv(1) == b'G'
os.execv(os.environ['REVIEW_REAL_GIT'],[os.environ['REVIEW_REAL_GIT'],*sys.argv[1:]])
''')
    hostgit.chmod(0o700)
    flag = tmp/'hold_push'
    env = {'PATH':str(tmp/'bin')+':'+os.environ['PATH'],'HOME':str(tmp/'home'),'CODEX_HOME':str(tmp/'user-codex'),
           'POLYCLAVE_PORT':'0','POLYCLAVE_STATE_DIR':str(tmp/'state'),'REVIEW_REAL_GIT':git,
           'REVIEW_PUSH_FLAG':str(flag),'REVIEW_BARRIER':str(listener.getsockname()[1])}
    process = subprocess.Popen([daemon],env=env,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
    try:
        line = process.stdout.readline().strip()
        endpoint,bearer = line.split('/#token=')
        def api(path, body=None):
            data = None if body is None else json.dumps(body).encode()
            req = urllib.request.Request(endpoint+'/api'+path,data,{'Content-Type':'application/json','Authorization':'Bearer '+bearer})
            with urllib.request.urlopen(req, timeout=8) as resp:
                return resp.status,json.load(resp)
        _, hub = api('/sessions',{'name':'test hub','repo_cwd':str(repo.resolve())})
        hubid = hub['id']
        _, msg = api(f'/sessions/{hubid}/messages',{'request_id':'scope','text':'Modify allowed.txt'})
        _, ticket = api(f'/sessions/{hubid}/tickets',{'message_id':msg['id'],'title':'scoped fixture','mandate':{'goal':'Modify allowed.txt','allowed_paths':['allowed.txt'],'forbidden_actions':[],'acceptance_criteria':[]},'max_runs':2})
        def dispatch(req):
            code, run = api('/runs',{'hub_session_id':hubid,'request_id':req,'vendor':'codex','repo_cwd':str(repo.resolve()),'instructions':'Modify allowed.txt','base_branch':'main','ticket_id':ticket['id']})
            assert code == 200
            return run['id']
        dbpath = tmp/'state'/'polyclave.sqlite'
        flag.touch()
        rid = dispatch('cancellation')
        with listener.accept()[0] as push, closing(sqlite3.connect(dbpath)) as db:
            push.settimeout(8)
            assert push.makefile('rb').read(len(b'push started')) == b'push started'
            code, cancel = api('/runs/'+rid+'/cancel', {})
            assert code == 200 and cancel['accepted']
            assert db.execute('select mcp_revoked from turns where run_id=?', (rid,)).fetchone() == (1,)
            assert cancel['broker_stopped'] is True
            # A read-side EOF proves the blocked push was stopped before the
            # route responded. The fixture never releases the pending push.
            assert push.recv(1) == b''
            assert db.execute(
                'select count(*) from results where run_id=? and accepted=1', (rid,)
            ).fetchone() == (0,)
        staging = tmp/'state'/'staging'/(rid+'-a1.git')
        refs = subprocess.check_output([git, '-C', str(staging), 'for-each-ref'], text=True)
        assert refs == '', refs
    finally:
        listener.close()
        process.terminate()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
