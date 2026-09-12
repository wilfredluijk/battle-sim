"""Shared connection arguments for bot command-line entry points."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import shlex
from typing import Dict
from urllib.parse import urlsplit


def add_connection_arguments(
    parser: argparse.ArgumentParser, *, default_url: str = "ws://localhost:7878/bot"
) -> None:
    """Add URL, local host/port and participant environment-file options.

    Credentials come from the explicitly selected file or BATTLE_BOT_TOKEN,
    never from a command-line token argument. Files are parsed as data; shell
    commands and variable expansion are not evaluated.
    """
    parser.set_defaults(_connection_default_url=default_url)
    group = parser.add_argument_group("server connection")
    group.add_argument(
        "--url", metavar="WS_URL",
        help=f"bot endpoint; otherwise BATTLE_SERVER_URL or {default_url}",
    )
    group.add_argument("--host", help="explicit ws:// host override (local default: localhost)")
    group.add_argument("--port", type=int, help="explicit ws:// port override (local default: 7878)")
    group.add_argument(
        "--env-file", metavar="PATH",
        help="participant file containing BATTLE_SERVER_URL and BATTLE_BOT_TOKEN",
    )


def _participant_environment(path: str) -> Dict[str, str]:
    try:
        with Path(path).open("rb") as source:
            content = source.read(16_385)
    except OSError:
        raise ValueError("could not read the participant --env-file") from None
    if len(content) > 16_384:
        raise ValueError("participant --env-file must be at most 16 KiB")
    try:
        lines = content.decode("utf-8").splitlines()
    except UnicodeError:
        raise ValueError("participant --env-file must use UTF-8") from None
    values: Dict[str, str] = {}
    for line_number, line in enumerate(lines, 1):
        try:
            fields = shlex.split(line, comments=True, posix=True)
        except ValueError:
            raise ValueError(f"invalid quoting in --env-file line {line_number}") from None
        if not fields:
            continue
        if fields[0] == "export":
            fields = fields[1:]
        if len(fields) != 1 or "=" not in fields[0]:
            raise ValueError(f"expected KEY=value in --env-file line {line_number}")
        key, value = fields[0].split("=", 1)
        if key not in {"BATTLE_SERVER_URL", "BATTLE_BOT_TOKEN"}:
            raise ValueError(f"unknown setting in --env-file line {line_number}")
        if key in values:
            raise ValueError(f"duplicate setting in --env-file line {line_number}")
        values[key] = value
    if not all(values.get(key, "").strip() for key in ("BATTLE_SERVER_URL", "BATTLE_BOT_TOKEN")):
        raise ValueError("--env-file must define BATTLE_SERVER_URL and BATTLE_BOT_TOKEN")
    return values


def connection_options(parser: argparse.ArgumentParser, args: argparse.Namespace) -> Dict[str, str]:
    """Resolve and validate keyword arguments for naval_sdk.run/run_async.

    An explicit URL or host/port overrides the chosen file/environment URL.
    A participant file replaces both environment settings as a pair, preventing
    accidental reuse of another team's ambient credential. Validation errors
    never echo file contents or a potentially credential-bearing URL.
    """
    if args.url is not None and (args.host is not None or args.port is not None):
        parser.error("use --url or --host/--port, not both")
    try:
        settings = _participant_environment(args.env_file) if args.env_file else os.environ
        if args.host is not None or args.port is not None:
            host = args.host if args.host is not None else "localhost"
            if ":" in host and not host.startswith("["):
                host = f"[{host}]"
            port = args.port if args.port is not None else 7878
            url = f"ws://{host}:{port}/bot"
        else:
            url = args.url if args.url is not None else settings.get("BATTLE_SERVER_URL") or args._connection_default_url
        try:
            parsed = urlsplit(url)
            valid = (
                parsed.scheme in {"ws", "wss"} and parsed.hostname
                and not any(char.isspace() for char in url)
                and not parsed.username and not parsed.password
                and not parsed.query and not parsed.fragment and parsed.path == "/bot"
                and (parsed.port is None or 1 <= parsed.port <= 65535)
            )
        except ValueError:
            valid = False
        if not valid:
            raise ValueError("use a ws:// or wss:// URL ending in /bot, with no credentials, query or fragment")
        token = settings.get("BATTLE_BOT_TOKEN", "")
        if parsed.scheme == "wss" and not token.strip():
            raise ValueError("a participant credential is required: use --env-file PATH or set BATTLE_BOT_TOKEN")
        return {"url": url, "token": token}
    except ValueError as error:
        parser.error(str(error))
        raise AssertionError("ArgumentParser.error must exit")  # pragma: no cover
