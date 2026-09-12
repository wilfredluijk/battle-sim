#!/usr/bin/env python3
import json
import subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]
script=r'''python3 - <<'CHECK'
import json,subprocess,shutil
p=json.loads(subprocess.check_output(['docker','inspect','battle-sim']))[0]
h=p['HostConfig']
assert p['Config']['User']=='10001:10001'
assert h['ReadonlyRootfs'] and h['CapDrop']==['ALL']
assert 'no-new-privileges:true' in h['SecurityOpt']
assert h['Memory']==1024**3 and h['PidsLimit']==128 and h['NanoCpus']==1500000000
assert h['PortBindings']['7878/tcp']==[{'HostIp':'127.0.0.1','HostPort':'7878'}]
assert [m['Destination'] for m in p['Mounts'] if m['RW']]==['/app/replays']
assert all(not x.startswith('BATTLE_ADMIN_PASSWORD=') for x in p['Config']['Env'])
assert p['State']['Health']['Status']=='healthy'
ssh=subprocess.check_output(['sshd','-T'],text=True)
assert 'passwordauthentication no' in ssh and 'kbdinteractiveauthentication no' in ssh
sidecars=list(__import__('pathlib').Path('/opt/battle-sim/replays').glob('*.build.json'))
assert sidecars and all(json.loads(f.read_text())['image_id'].startswith('sha256:') for f in sidecars)
print(json.dumps({'container':'healthy','user':p['Config']['User'],'root_read_only':True,'capabilities':'all dropped','memory_limit_bytes':h['Memory'],'cpu_limit':h['NanoCpus']/1e9,'pid_limit':h['PidsLimit'],'public_application_port':False,'writable_mounts':['/app/replays'],'disk_free_bytes':shutil.disk_usage('/opt/battle-sim').free,'image':p['Config']['Image'],'image_id':p['Image'],'ssh_password_login':False,'replay_build_sidecars':len(sidecars)}))
CHECK'''
result=subprocess.check_output([str(root/'deploy/ssh.sh'),script],text=True)
(root/'release-artifacts/host-verification.json').write_text(result)
print(result)
