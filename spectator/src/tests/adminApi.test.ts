import { afterEach, describe, expect, it, vi } from 'vitest';
import * as api from '../lib/adminApi';

/** Build a minimal `Response` stand-in for the REST client to consume. */
function jsonResponse(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as Response;
}

describe('adminApi', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('login posts the password and returns the token', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse(200, { token: 'jwt-token', expires_at: 42 }));
    vi.stubGlobal('fetch', fetchMock);

    const result = await api.login('hunter2');
    expect(result.token).toBe('jwt-token');

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/login');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({ password: 'hunter2' });
  });

  it('login throws a typed ApiError on 401', async () => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          jsonResponse(401, { code: 'invalid_credentials', message: 'incorrect password' }),
        ),
    );

    await expect(api.login('wrong')).rejects.toMatchObject({
      name: 'ApiError',
      status: 401,
      code: 'invalid_credentials',
      message: 'incorrect password',
    });
  });

  it('fetchRoom parses the room payload', async () => {
    const room = {
      room: 'main',
      state: 'lobby',
      tick: 0,
      bots: [],
      config: { hull_hp: 100 },
    };
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(200, room)));

    const info = await api.fetchRoom();
    expect(info.state).toBe('lobby');
    expect(info.config.hull_hp).toBe(100);
  });

  it('fetchReport returns null on 404 (no match finished yet)', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(404, { code: 'no_report' })),
    );
    expect(await api.fetchReport()).toBeNull();
  });

  it('fetchConfigSchema unwraps the fields array', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(200, { fields: [{ key: 'hull_hp' }] })),
    );
    const fields = await api.fetchConfigSchema();
    expect(fields).toHaveLength(1);
  });

  it('startMatch sends the bearer token and POSTs the right path', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(204, {}));
    vi.stubGlobal('fetch', fetchMock);

    await api.startMatch('tok-123');

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/room/start');
    expect(init.method).toBe('POST');
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer tok-123');
  });

  it('putConfig serialises the config and sends the token', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(204, {}));
    vi.stubGlobal('fetch', fetchMock);

    await api.putConfig('tok-9', { hull_hp: 250 });

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/room/config');
    expect(init.method).toBe('PUT');
    expect(JSON.parse(init.body as string)).toEqual({ hull_hp: 250 });
  });
});
