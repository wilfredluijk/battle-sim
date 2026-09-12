#!/usr/bin/env python3
"""Restart acceptance check. Run between matches; intentionally interrupts a test match."""
import asyncio
import importlib.util
import json
from pathlib import Path
import subprocess
import time

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('verify',ROOT/'deploy/verify-public.py')
v=importlib.util.module_from_spec(spec);spec.loader.exec_module(v)

async def main():
    _,login=v.request(v.ADMIN+'/api/login',method='POST',data={'password':(v.SECRETS/'admin-password').read_text().strip()})
    old=login['token']
    roster=json.loads((v.SECRETS/'participants.json').read_text())
    await asyncio.sleep(3)
    ws,welcome=await v.connect(roster[0]['token'])
    await ws.send(json.dumps({'type':'ready','config_hash':welcome['config_hash']}))
    await asyncio.sleep(.2)
    assert v.request(v.ADMIN+'/api/room/start',old,'POST')[0]==204
    while (await v.recv(ws))['type']!='tick': pass
    before=subprocess.check_output([str(ROOT/'deploy/ssh.sh'),'find /opt/battle-sim/replays -maxdepth 1 -name "*.jsonl" -printf "%f\\n"'],text=True).splitlines()
    start=time.monotonic()
    stop=subprocess.check_output([str(ROOT/'deploy/ssh.sh'),"docker stop --time 12 battle-sim >/dev/null && docker inspect battle-sim --format '{{.State.ExitCode}}'"],text=True).strip()
    assert stop=='0',stop
    subprocess.run([str(ROOT/'deploy/ssh.sh'),'docker start battle-sim >/dev/null'],check=True)
    for _ in range(60):
        try:
            if v.request(v.ADMIN+'/healthz')[0]==200: break
        except OSError: pass
        await asyncio.sleep(.5)
    else: raise AssertionError('restart health timeout')
    assert v.request(v.ADMIN+'/api/room',old)[0]==401,'old admin JWT survived restart'
    _,login=v.request(v.ADMIN+'/api/login',method='POST',data={'password':(v.SECRETS/'admin-password').read_text().strip()})
    _,room=v.request(v.ADMIN+'/api/room',login['token'])
    assert room['state']=='lobby' and not room['bots'],room
    after=subprocess.check_output([str(ROOT/'deploy/ssh.sh'),'find /opt/battle-sim/replays -maxdepth 1 -name "*.jsonl" -printf "%f\\n"'],text=True).splitlines()
    assert set(before)<=set(after),'restart removed replay files'
    await ws.close()
    result={'sigterm_exit_code':0,'restart_seconds':round(time.monotonic()-start,2),'old_admin_token':'rejected','room_after_restart':'empty lobby','replays':'preserved','interrupted_match':'not resumed'}
    (ROOT/'release-artifacts/restart-verification.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result))
asyncio.run(main())
