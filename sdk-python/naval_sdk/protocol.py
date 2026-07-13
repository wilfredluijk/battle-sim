"""Typed views of the wire protocol.

These dataclasses mirror the messages documented in `docs/PROTOCOL.md`. They are
*derived* from raw dicts that arrive over the WebSocket — bot authors are free to
ignore them and read raw frames via `Bot.raw_recv()` / `Bot.raw_send()`.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional, Sequence, Tuple

from .helpers import Point, bearing_to, lead_target

SensorMode = Literal["active", "passive"]
ContactKind = Literal["ship", "shell", "unknown"]


@dataclass(frozen=True)
class ShipSpecs:
    max_forward_speed: float
    max_reverse_speed: float
    acceleration: float
    turn_rate_deg_per_s: float
    hull_hp: int
    max_ammo: int
    gun_cooldown_ticks: int
    hit_radius: float
    shell_speed: float
    max_shell_range: float
    splash_radius: float
    max_splash_damage: int

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "ShipSpecs":
        return cls(
            max_forward_speed=float(d["max_forward_speed"]),
            max_reverse_speed=float(d["max_reverse_speed"]),
            acceleration=float(d["acceleration"]),
            turn_rate_deg_per_s=float(d["turn_rate_deg_per_s"]),
            hull_hp=int(d["hull_hp"]),
            max_ammo=int(d["max_ammo"]),
            gun_cooldown_ticks=int(d["gun_cooldown_ticks"]),
            hit_radius=float(d["hit_radius"]),
            shell_speed=float(d["shell_speed"]),
            max_shell_range=float(d["max_shell_range"]),
            splash_radius=float(d["splash_radius"]),
            max_splash_damage=int(d["max_splash_damage"]),
        )


@dataclass(frozen=True)
class MapInfo:
    width: int
    height: int

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "MapInfo":
        return cls(width=int(d["width"]), height=int(d["height"]))


@dataclass(frozen=True)
class Welcome:
    bot_id: str
    ship_id: str
    map: MapInfo
    tick_hz: int
    ship_specs: ShipSpecs
    #: Powerup ids the server understands. Forward-compatible: unknown future entries
    #: are simply passed back to the server if a bot picks them — the server validates.
    available_powerups: List[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "Welcome":
        return cls(
            bot_id=str(d["bot_id"]),
            ship_id=str(d["ship_id"]),
            map=MapInfo.from_dict(d["map"]),
            tick_hz=int(d["tick_hz"]),
            ship_specs=ShipSpecs.from_dict(d["ship_specs"]),
            available_powerups=[str(p) for p in d.get("available_powerups", [])],
        )


@dataclass(frozen=True)
class PowerupStatus:
    """Live status for one of the bot's picked powerups, mirrored in every tick."""

    id: str
    used: bool
    active_ticks_left: int

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "PowerupStatus":
        return cls(
            id=str(d["id"]),
            used=bool(d.get("used", False)),
            active_ticks_left=int(d.get("active_ticks_left", 0)),
        )


@dataclass(frozen=True)
class GameStart:
    tick: int
    starting_position: Tuple[float, float]
    starting_heading_deg: float

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "GameStart":
        pos = d["starting_position"]
        return cls(
            tick=int(d["tick"]),
            starting_position=(float(pos[0]), float(pos[1])),
            starting_heading_deg=float(d["starting_heading_deg"]),
        )


@dataclass(frozen=True)
class SelfState:
    pos: Tuple[float, float]
    heading_deg: float
    speed: float
    hp: int
    ammo: int
    rudder: float
    throttle: float
    #: Loadout the bot picked for the match, in pick order. Empty if the bot never sent
    #: `select_powerups`.
    selected_powerups: Tuple[str, ...] = ()
    #: One entry per picked powerup, same order. Use this to check whether a powerup is
    #: still available, currently active, or already used.
    powerup_status: Tuple[PowerupStatus, ...] = ()

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "SelfState":
        pos = d["pos"]
        return cls(
            pos=(float(pos[0]), float(pos[1])),
            heading_deg=float(d["heading_deg"]),
            speed=float(d["speed"]),
            hp=int(d["hp"]),
            ammo=int(d["ammo"]),
            rudder=float(d["rudder"]),
            throttle=float(d["throttle"]),
            selected_powerups=tuple(str(p) for p in d.get("selected_powerups", [])),
            powerup_status=tuple(
                PowerupStatus.from_dict(s) for s in d.get("powerup_status", [])
            ),
        )

    # ---- Convenience for bots ---------------------------------------------

    def powerup(self, powerup_id: str) -> Optional[PowerupStatus]:
        """Look up the live status of a specific picked powerup, or `None` if not picked."""
        for status in self.powerup_status:
            if status.id == powerup_id:
                return status
        return None

    def powerup_ready(self, powerup_id: str) -> bool:
        """True iff the bot picked this powerup and has not yet activated it."""
        status = self.powerup(powerup_id)
        return status is not None and not status.used

    def powerup_active(self, powerup_id: str) -> bool:
        """True iff this powerup is currently in effect (`active_ticks_left > 0`)."""
        status = self.powerup(powerup_id)
        return status is not None and status.active_ticks_left > 0


@dataclass(frozen=True)
class Contact:
    id: str
    kind: ContactKind
    pos: Tuple[float, float]
    bearing_deg: float
    range: Optional[float]
    confidence: float

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "Contact":
        pos = d["pos"]
        rng = d.get("range")
        return cls(
            id=str(d["id"]),
            kind=d.get("kind", "unknown"),
            pos=(float(pos[0]), float(pos[1])),
            bearing_deg=float(d["bearing_deg"]),
            range=None if rng is None else float(rng),
            confidence=float(d.get("confidence", 0.0)),
        )


@dataclass(frozen=True)
class HitEvent:
    amount: int


@dataclass(frozen=True)
class ShellSplashEvent:
    pos: Tuple[float, float]


@dataclass(frozen=True)
class PowerupActivatedEvent:
    """Reported when a ship activates a powerup. Always emitted for the bot's own
    activations (``contact_id`` is ``None`` — you are not a contact to yourself); emitted
    for another ship only when that ship shows up in this bot's sensor sweep this tick, in
    which case ``contact_id`` is the same per-tick anonymized ``c_<n>`` id it appears under
    in ``contacts``. Never the ground-truth ship id — the event is re-anonymized every
    tick, so it can't be used to track a specific opponent across ticks."""

    contact_id: Optional[str]
    powerup: str


TickEvent = Any  # HitEvent | ShellSplashEvent | PowerupActivatedEvent | unknown


def _parse_event(d: Dict[str, Any]) -> TickEvent:
    try:
        kind = d.get("type")
        if kind == "hit":
            return HitEvent(amount=int(d["amount"]))
        if kind == "shell_splash":
            pos = d["pos"]
            return ShellSplashEvent(pos=(float(pos[0]), float(pos[1])))
        if kind == "powerup_activated":
            raw_contact = d.get("contact_id")
            return PowerupActivatedEvent(
                contact_id=None if raw_contact is None else str(raw_contact),
                powerup=str(d["powerup"]),
            )
    except (AttributeError, KeyError, IndexError, TypeError, ValueError):
        # A known event type arrived malformed (missing/ill-typed field). Fall
        # through to the raw-dict path so one bad event can't sink the whole tick.
        return d
    return d  # forward-compatible: unknown event types stay as raw dicts


@dataclass(frozen=True)
class WorldView:
    """Bot-side view of a single `tick` message."""

    tick: int
    deadline_ms: int
    self_state: SelfState
    contacts: List[Contact]
    events: List[TickEvent]

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "WorldView":
        return cls(
            tick=int(d["tick"]),
            deadline_ms=int(d["deadline_ms"]),
            self_state=SelfState.from_dict(d["self"]),
            contacts=[Contact.from_dict(c) for c in d.get("contacts", [])],
            events=[_parse_event(e) for e in d.get("events", [])],
        )

    # Convenience accessors -------------------------------------------------

    @property
    def me(self) -> SelfState:
        return self.self_state

    def nearest_contact(self) -> Optional[Contact]:
        ranged = [c for c in self.contacts if c.range is not None]
        if not ranged:
            return None
        return min(ranged, key=lambda c: c.range)  # type: ignore[arg-type,return-value]


@dataclass(frozen=True)
class GameOver:
    winner: Optional[str]
    final_tick: int
    replay_id: str

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "GameOver":
        return cls(
            winner=d.get("winner"),
            final_tick=int(d["final_tick"]),
            replay_id=str(d["replay_id"]),
        )


# ---------------------------------------------------------------------------
# Outbound
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class FireCommand:
    bearing_deg: float
    range: float

    def to_dict(self) -> Dict[str, Any]:
        return {"bearing_deg": float(self.bearing_deg), "range": float(self.range)}


@dataclass
class Command:
    """A bot's response to a single `tick`.

    `throttle` and `rudder` are clamped to `[-1, 1]` by the server. `sensor_mode`
    defaults to `"active"`. To fire, either pass `fire=FireCommand(...)` directly
    or call `Command.fire_at(target_pos)` / `.fire_at(target_pos, target_vel, ...)`.

    To activate one of your picked powerups, set `activate_powerup="overdrive"` (or any
    other id from `welcome.available_powerups`). The server validates that the powerup
    is in your loadout and hasn't been used yet — a bad id earns a typed `error` frame.
    """

    throttle: float = 0.0
    rudder: float = 0.0
    sensor_mode: SensorMode = "active"
    fire: Optional[FireCommand] = None
    activate_powerup: Optional[str] = None

    def fire_at(
        self,
        target_pos: Point,
        *,
        shooter_pos: Optional[Point] = None,
        target_vel: Optional[Sequence[float]] = None,
        shell_speed: float = 70.0,
        range: Optional[float] = None,
        lead: bool = True,
    ) -> "Command":
        """Aim a shell at `target_pos`.

        If `shooter_pos` is supplied and `lead=True` and `target_vel` is non-zero,
        the SDK leads the target using `shell_speed` (default 70, matching the
        server's `ship_specs.shell_speed`). Prefer passing the value you read
        from `welcome.ship_specs.shell_speed` so your lead math tracks any
        future balance changes. When `shooter_pos` is omitted, the bearing is
        computed from the origin — pass `shooter_pos=view.me.pos` to do it
        right.

        `range` defaults to the distance from `shooter_pos` to the aim point,
        clamped server-side to `max_shell_range`.
        """
        origin = tuple(shooter_pos) if shooter_pos is not None else (0.0, 0.0)
        aim: Tuple[float, float] = (float(target_pos[0]), float(target_pos[1]))
        if lead and target_vel is not None and (target_vel[0] != 0.0 or target_vel[1] != 0.0):
            predicted = lead_target(origin, aim, target_vel, shell_speed)
            if predicted is not None:
                aim = predicted

        bearing = bearing_to(origin, aim)
        if range is None:
            dx = aim[0] - origin[0]
            dy = aim[1] - origin[1]
            range_val = (dx * dx + dy * dy) ** 0.5
        else:
            range_val = float(range)

        self.fire = FireCommand(bearing_deg=bearing, range=range_val)
        return self

    def to_dict(self, tick: int) -> Dict[str, Any]:
        out: Dict[str, Any] = {
            "type": "command",
            "tick": int(tick),
            "throttle": float(self.throttle),
            "rudder": float(self.rudder),
            "sensor_mode": self.sensor_mode,
        }
        if self.fire is not None:
            out["fire"] = self.fire.to_dict()
        if self.activate_powerup is not None:
            out["activate_powerup"] = str(self.activate_powerup)
        return out
