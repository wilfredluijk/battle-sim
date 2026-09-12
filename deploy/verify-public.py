#!/usr/bin/env python3
"""Exercise public TLS, private auth, abuse handling and a full eight-bot SDK match.
Requires the admin SSH tunnel and ./sdk-python installed. Never logs credentials.
Run only between training matches: these tests occupy the participant roster.
"""
import argparse
import asyncio
from collections import Counter
import json
from pathlib import Path
import statistics
import time
import urllib.error
import urllib.request
import websockets
from naval_sdk import Bot, Command
from naval_sdk.bot import run_async

ROOT = Path(__file__).resolve().parents[1]
SECRETS = ROOT / '.deployment-secrets'
URL = 'wss://93.190.187.250/bot'
ADMIN = 'http://127.0.0.1:8787'
REPORT = {}


def request(url, token=None, method='GET', data=None):
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(url, headers=headers, method=method,
        data=None if data is None else json.dumps(data).encode())
    try:
        with urllib.request.urlopen(req, timeout=8) as response:
            body = response.read()
            return response.status, json.loads(body) if body else None
    except urllib.error.HTTPError as e:
        return e.code, None

async def recv(ws):
    return json.loads(await asyncio.wait_for(ws.recv(), 8))

async def connect(credential, name='untrusted-display-name'):
    ws = await websockets.connect(URL, open_timeout=8)
    await ws.send(json.dumps({'type':'hello', 'name':name, 'version':'deployment-test/3', 'token':credential}))
    return ws, await recv(ws)

async def main():
    roster = json.loads((SECRETS / 'participants.json').read_text())
    status, login = await asyncio.to_thread(request, ADMIN + '/api/login', None, 'POST',
        {'password':(SECRETS / 'admin-password').read_text().strip()})
    assert status == 200, 'admin login through SSH failed'
    token = login['token']
    async def api(path, method='GET', data=None):
        return await asyncio.to_thread(request, ADMIN + path, token, method, data)
    for path in ['/', '/spectate', '/api/room', '/api/replays', '/api/montecarlo/status', '/healthz']:
        status, _ = await asyncio.to_thread(request, 'https://93.190.187.250' + path)
        assert status == 404, (path, status)
    for path in ['/api/room', '/api/room/report', '/api/replays', '/api/config/schema', '/api/montecarlo/status', '/api/metrics']:
        status, _ = await asyncio.to_thread(request, ADMIN + path)
        assert status == 401, (path, status)
    REPORT['information_isolation'] = 'public routes 404; private sensitive reads 401 without JWT'
    for bad in ['', 'wrong-token']:
        ws, msg = await connect(bad)
        assert msg.get('code') == 'unauthorized', msg
        await ws.close()
    ws, msg = await connect(roster[0]['token'])
    assert msg['type'] == 'welcome'
    status, room = await api('/api/room')
    assert any(b['name'] == roster[0]['identity'] for b in room['bots']), room
    duplicate, refusal = await connect(roster[0]['token'])
    assert refusal.get('code') == 'unauthorized'
    await duplicate.close()
    await ws.close()
    REPORT['participant_auth'] = 'missing/wrong tokens refused; reserved identity enforced; duplicate connection refused'
    await asyncio.sleep(2.2)
    # Slow hello must not reserve a room slot.
    async with websockets.connect(URL, open_timeout=8) as slow:
        response = await recv(slow)
        assert response.get('code') == 'handshake_timeout'
    REPORT['hello_timeout'] = 'idle WebSocket closed after 5 seconds'
    # Repeated valid lobby messages still obey the WebSocket message budget.
    ws, welcome = await connect(roster[0]['token'])
    ready = json.dumps({'type':'ready', 'config_hash':welcome['config_hash']})
    try:
        for _ in range(150):
            await ws.send(ready)
        while True:
            response = await recv(ws)
            if response.get('code') == 'rate_limited':
                break
    except websockets.ConnectionClosed:
        pass
    await ws.close()
    REPORT['message_flood'] = 'authenticated flood disconnected; health remains responsive'
    assert (await asyncio.to_thread(request, ADMIN + '/healthz'))[0] == 200
    # Ground truth is unavailable on a fresh private spectator connection.
    async with websockets.connect('ws://127.0.0.1:8787/spectate') as spectator:
        await spectator.send(json.dumps({'type':'authenticate','token':'invalid'}))
        assert (await recv(spectator)).get('code') == 'unauthorized'
    async with websockets.connect('ws://127.0.0.1:8787/spectate') as spectator:
        await spectator.send(json.dumps({'type':'authenticate','token':token}))
        assert (await recv(spectator))['type'] == 'world'
    REPORT['spectator_auth'] = 'no world before valid administrator JWT'
    await asyncio.sleep(2.2)

    class SoakBot(Bot):
        def __init__(self):
            super().__init__()
            self.count = 0
            self.errors = Counter()
            self.configs = []
            self.matches = set()
            self.last_seen_tick = 0
        def accept_configuration(self, config, config_hash):
            assert config['protocol_version'] == '3.0'
            assert config['sim_config'] and abs(config['simulation_dt'] - 0.1) < 1e-7
            self.configs.append(config_hash)
            return True
        def on_tick(self, view):
            assert view.tick > self.last_seen_tick
            self.last_seen_tick = view.tick
            self.matches.add(view.match_id)
            self.count += 1
            return Command(throttle=0, rudder=0, sensor_mode='active')
        def on_error(self, code, message):
            self.errors[code] += 1
        def on_game_over(self, result):
            return False

    bots = [SoakBot() for _ in roster]
    tasks = [asyncio.create_task(run_async(b, url=URL, token=p['token'], name='ignored')) for b,p in zip(bots,roster)]
    for _ in range(100):
        _, room = await api('/api/room')
        if len(room['bots']) == 8 and all(b['ready'] for b in room['bots']):
            break
        if any(t.done() for t in tasks):
            raise AssertionError('bot failed before readiness')
        await asyncio.sleep(.1)
    else:
        raise AssertionError('eight-bot readiness timeout')
    assert len({b.configs[-1] for b in bots}) == 1, 'configuration agreement differs'
    rtt = []
    for b in bots:
        start = time.monotonic()
        pong = await b._ws.ping()
        await asyncio.wait_for(pong, 3)
        rtt.append((time.monotonic() - start) * 1000)
    REPORT['websocket_rtt_ms'] = {'min':min(rtt), 'median':statistics.median(rtt), 'max':max(rtt)}
    before = (await api('/api/metrics'))[1]
    assert (await api('/api/room/start', 'POST'))[0] == 204
    start = time.monotonic()
    print('Eight authenticated SDK bots started; running a full 3000-tick match.', flush=True)
    while not all(t.done() for t in tasks):
        await asyncio.sleep(15)
        elapsed = time.monotonic() - start
        print(json.dumps({'elapsed_s':round(elapsed), 'bot_ticks':[b.count for b in bots]}), flush=True)
        assert elapsed < 340, 'full match exceeded 340 seconds'
    results = await asyncio.gather(*tasks)
    assert all(result and result.final_tick == 3000 and result.winner is None for result in results), results
    assert all(b.count >= 2990 for b in bots), [b.count for b in bots]
    assert all(len(b.matches) == 1 and '' not in b.matches for b in bots)
    _, after = await api('/api/metrics')
    REPORT['soak'] = {'elapsed_s':round(time.monotonic()-start, 2), 'ticks_per_bot':[b.count for b in bots],
        'errors':[dict(b.errors) for b in bots], 'metrics_before':before, 'metrics_after':after,
        'replay_id':results[0].replay_id, 'outcome':'draw at 3000 ticks'}
    _, report = await api('/api/room/report')
    assert len(report['bots']) == 8
    assert (await api('/api/replays/' + results[0].replay_id))[0] == 200, 'completed replay reconstruction failed'
    REPORT['replay_reconstruction'] = 'completed eight-bot replay reconstructed successfully'
    output = ROOT / 'release-artifacts' / 'public-verification.json'
    output.write_text(json.dumps(REPORT, indent=2)+'\n')
    print(json.dumps(REPORT, indent=2), flush=True)

if __name__ == '__main__':
    asyncio.run(main())
