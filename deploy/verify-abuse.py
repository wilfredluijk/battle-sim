#!/usr/bin/env python3
"""Post-soak checks. Requires the private admin tunnel; leaves the room in its lobby."""
import asyncio
import importlib.util
import json
from pathlib import Path
import shlex
import subprocess
import time
import websockets

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('verification', ROOT / 'deploy/verify-public.py')
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)

async def main():
    roster = json.loads((v.SECRETS/'participants.json').read_text())
    credential = roster[0]
    _, login = v.request(v.ADMIN+'/api/login', data={'password':(v.SECRETS/'admin-password').read_text().strip()}, method='POST')
    token = login['token']
    def api(path, method='GET', data=None): return v.request(v.ADMIN+path, token, method, data)
    await asyncio.sleep(3)
    ws, welcome = await v.connect(credential['token'])
    await ws.send(json.dumps({'type':'ready','config_hash':welcome['config_hash']}))
    await asyncio.sleep(.2)
    assert api('/api/room/start', 'POST')[0] == 204
    while True:
        msg = await v.recv(ws)
        if msg['type'] == 'tick': break
    base = {'type':'command', 'match_id':msg['match_id'], 'tick':msg['tick'], 'throttle':0,'rudder':0,'sensor_mode':'passive'}
    await ws.send(json.dumps({**base, 'match_id':'previous-match'}))
    await ws.send(json.dumps({**base, 'tick':base['tick']+1}))
    await ws.send(json.dumps(base))
    await ws.send(json.dumps(base))
    errors = set()
    for _ in range(20):
        msg = await v.recv(ws)
        if msg['type'] == 'error': errors.add(msg['code'])
        if {'wrong_match','wrong_tick','duplicate_command'} <= errors: break
    assert {'wrong_match','wrong_tick','duplicate_command'} <= errors, errors
    assert api('/api/room/abort', 'POST')[0] == 204
    await ws.close()
    await asyncio.sleep(3)
    ws, welcome = await v.connect(credential['token'])
    assert welcome['type'] == 'welcome'
    # Atomically replace the mounted roster file so ongoing sessions see revocation.
    def enabled(value):
        code = '''import json,os
from pathlib import Path
p=Path('/opt/battle-sim/secrets/participants.json')
r=json.loads(p.read_text())
for entry in r:
 if entry['identity']==%r: entry['enabled']=%r
t=p.with_suffix('.new');t.write_text(json.dumps(r));os.chown(t,0,10001);os.chmod(t,0o440);t.replace(p)
''' % (credential['identity'], value)
        subprocess.run([str(ROOT/'deploy/ssh.sh'), 'python3 -c '+shlex.quote(code)], check=True)
    try:
        enabled(False)
        try:
            await asyncio.wait_for(ws.recv(), 8)
            raise AssertionError('revoked connection remained active')
        except websockets.ConnectionClosed:
            pass
    finally:
        enabled(True)
        await ws.close()
    await asyncio.sleep(2.2)
    ws, welcome = await v.connect(credential['token'])
    assert welcome['type'] == 'welcome'
    await ws.close()
    await asyncio.sleep(2.2)
    # Stop reading entirely: the server must evict the silent connection on heartbeat expiry.
    ws, welcome = await v.connect(credential['token'])
    assert welcome['type'] == 'welcome'
    ws.transport.pause_reading()
    await asyncio.sleep(22)
    _, room = api('/api/room')
    assert not room['bots'], 'silent reader retained an active room slot'
    ws.transport.resume_reading()
    await ws.close()
    result={'exact_command_admission':sorted(errors),'live_revocation':'active connection closed; restored token reconnects','slow_reader':'non-reading connection evicted within 22 seconds'}
    (ROOT/'release-artifacts/abuse-verification.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result),flush=True)

asyncio.run(main())
