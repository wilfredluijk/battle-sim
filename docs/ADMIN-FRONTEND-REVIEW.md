# Admin frontend review

Reviewed 12 September 2026 for an eight-team training in Amersfoort.

The frontend has a useful foundation: a restrained naval palette, live HP/ammo/control telemetry, recorded matches, replay speed controls, event markers and per-bot replay perspectives. The next improvement should make the session easier to operate and explain to a room of participants.

## Review evidence

Inspected the Svelte source and the existing local server build in Chrome. Exercised login, the empty lobby, an eight-bot lobby, a live match, both battlefield layouts, an aborted-match report, the replay list and replay viewer. The local instance used tournament mode and eight example bots. No production changes were made. The visual observations below come from a 1,853 × 905 CSS-pixel viewport; smaller screen behavior was assessed from CSS, not a full device audit.

## Fix before the training

| Priority | Finding | Evidence | Proposed behavior |
|---|---|---|---|
| High | The live battlefield overflows the viewport. | With split view, the main content had 856 px of height but the canvas and sidebar each measured 1,430 px. Document height was 1,495 px. The full view also visibly overflowed. | Constrain the grid row and canvas to available height, preserve the map aspect ratio inside that region, and give the roster its own bounded space. All map edges must remain visible without page scrolling. |
| High | Eight detailed cards bury essential information. | Cards were 140 px tall; the event section started at y=1,346, below the 905 px viewport. | Compact rows showing team, HP, ammo and status; expand telemetry only for the selected bot. Keep a short, independently visible event feed. |
| High | Authentication is described as a connection failure. | Before login: “Cannot reach the server: administrator authentication required”, alongside “Loading parameters…”. | A focused login screen. Distinct states for sign-in required, session expired, connecting, disconnected and stale data. Stop protected polling until authenticated. |
| High | Report language can misrepresent the result. | The report displayed 150% and 133% “Accuracy”. The server divides hit events by shots; one shot can produce several hit events. The draw branch always says “no survivors”, although timeout ties can have survivors. | Rename the existing metric to “Hits per shot”, or add a distinct-shots-hit metric for conventional accuracy. Do not clamp to 100%. Use “Draw” until a recorded end reason supports more specific wording. |
| High | Destructive actions lack reliable feedback. | Abort is immediate and errors are swallowed in Topbar; reset hides the report even on failure; live kick has no local pending/error handling. | Name the affected match/team in an explicit confirmation for abort and mid-match forfeit. Show pending, success and failure states; keep the current screen when an action fails. Avoid confirmation dialogs for harmless navigation. |
| Medium | Tournament mode offers an unavailable action. | Topbar exposes Monte Carlo whenever replay mode is false; the server rejects Monte Carlo in tournament mode. | Publish server capabilities to the admin API and hide or explain unavailable controls. Put offline analysis under a separate analysis surface. |
| Medium | Draft rules can be confused with active rules. | ConfigForm is seeded once, has no dirty indicator, and Start match is independent of its edited values. | Show “3 unapplied changes”, active preset, a change summary and a sticky save/discard bar. Starting with a draft should require applying or discarding it. Explain that applying changes requires bots to acknowledge the updated rules. |
| Medium | A retained hull is treated as a connected bot. | worldFrame marks every ship in a frame connected; the server retains forfeited hulls. | Supply explicit connection/forfeit state and reason to the admin surface. Distinguish destroyed, disconnected and forfeited; do not infer network status from ship presence. |

Source entry points: `spectator/src/app.css`, `components/PreMatch.svelte`, `components/ConfigForm.svelte`, `components/Topbar.svelte`, `components/BotCard.svelte`, `components/Report.svelte`, `lib/worldFrame.ts`; server reporting and forfeit handling in `server/src/room.rs`.

## Proposed layout and styling

Keep the existing dark blue identity. Introduce stronger hierarchy through size and spacing: roughly 15–16 px body text, 13–14 px secondary labels, 24–28 px screen titles and a large match clock. These are design targets, not accessibility thresholds. Reserve blue emphasis for the primary action and selection; use red for actions with consequences. Logout should be a quiet account action.

Use persistent navigation: **Lobby · Live match · Results · Replays**, with Settings secondary and offline Analysis shown only where supported. Keep the viewed screen separate from the server's live state: “Replay · paused” must not look like a live match just because the background connection is healthy.

The lobby should devote most space to the eight expected teams and readiness blockers. A narrower panel summarizes active rules, map, duration and command deadline. Put detailed physics/balance inputs in an expandable settings area with units, explanations, presets and import/export. Show absent teams as well as connected ones; never expose participant tokens in the shared screen.

The live screen should have a stable header showing round, elapsed/remaining time, survivors and connection freshness. The battlefield takes the remaining main region. Use a compact roster beside it; selecting a team highlights its ship and reveals speed, rudder, sensor mode and named powerups with remaining duration. Keep rows stable while a match runs. Let operators explicitly sort when needed rather than moving rows under their pointer.

Provide optional radar rings, trails and labels, plus a legend. The current name-to-palette hash permits color collisions: Team Atlas, Harbour and Echo shared blue in the sample. Assign distinct session colors where possible and pair them with short team identifiers or shapes.

Add a projector preset with larger labels, clear HP bars and a persistent clock. Hiding admin controls is a presentation preference, not an authorization boundary. A separately accessible audience page requires its own restricted spectator permissions and an explicit decision about when ground-truth information may be shown.

For accessibility, verify normal text at 4.5:1 contrast, visible keyboard focus, labeled login input and status announcements for actions. Prefer comfortable 36–44 px controls for the trainer; WCAG's minimum target-size criterion is 24 × 24 CSS px with exceptions including spacing. Review the live layout, header and replay controls at 1,366 × 768, tablet width and 200% zoom; the existing narrow-screen rule primarily addresses the lobby.

Sources: [W3C text contrast guidance](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum), [W3C target size guidance](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum).

## Information to add

| Where | Information | Data requirements |
|---|---|---|
| Lobby | Expected/connected/ready counts, missing team names, readable readiness blocker, acknowledged rules | Expected roster and acknowledgement diagnostics need a sanitized admin API. Never return credentials. |
| Match header | Clock, survivors, round label, data freshness | Clock/survivors mostly derive from existing data; publish timeout/timing configuration rather than hardcoding 3,000 ticks or 10 Hz. Round labels need session metadata. |
| Trainer health panel | Received commands, rejects, recent simulation timing | Aggregate counters already exist at `/api/metrics`. Derive interval changes rather than presenting lifetime counters as current rates. |
| Per-team diagnostics | Round-trip latency, response-time percentiles, late-command rate and disconnect reason | Requires new measurement and reporting. Browser-to-server latency is not bot-to-server latency. Existing aggregate rejection counts mix reasons and must not be labeled “late commands”. |
| Events | Team names, readable timestamps, kills/hits/powerups/disconnect filters | Existing events are formatted as strings with IDs such as `s_3`. Preserve structured events and resolve names. Additional reasons may require server fields. |
| Report | Explicit end reason, useful ranking, damage visualization, watch replay, export | Existing stats support a simple debrief. End reason, formal standings and export metadata require additions. Define competition scoring before labeling a sort order as rank. |
| Replay library | Round/date/team labels, outcome, duration, search and filters | Current list joins all bot names and shows “Draw / aborted / incomplete” together. Store distinct outcome metadata; older replays should say “Unknown” when necessary. |

The deployment validation observed command deadline misses. A pre-training connection check and honest timing diagnostics would therefore be especially valuable for the Amersfoort venue. Report fresh venue measurements rather than treating the prior VPS test as a measurement of the training network.

## Features worth adding

1. **Training session manager:** name rounds, track the expected roster, show readiness, offer a cancellable start countdown, preserve results and prepare the next round. This removes repeated bookkeeping during teaching.
2. **Replay debrief tools:** build on the existing speed controls, markers and perspective selector with previous/next event, frame stepping, clickable event names, bookmarks and selected-bot telemetry. A side-by-side ground-truth versus sensor view would help explain why a bot made a decision.
3. **Team onboarding and connection check:** copy the public server address, show SDK/protocol requirements and run a short preflight. Keep each team's credential distribution separate from the projected admin screen.
4. **Session standings and exports:** accumulate results under a clearly defined scoring rule and export CSV/JSON. Distinguish training statistics from official competition scoring.
5. **Presenter controls:** projector typography, selected-team focus and optional countdown/elimination sounds with mute. Start with a trainer-controlled display; a public spectator service is a separate feature.
6. **Advanced analysis later:** compare bot versions, movement/shot heatmaps and multi-run results in offline analysis. The existing Monte Carlo tools provide a foundation without changing tournament rules.

## Suggested implementation sequence

**First:** map fit, compact roster, login/stale-data states, truthful report labels, reliable action feedback and capability-aware navigation. These improve correctness and operation immediately.

**Second:** the lobby/rules redesign, visible match clock, readable event feed, replay link from results and projector preset. Most can use existing data with small API additions for timing/capabilities.

**Third:** session persistence, expected roster, per-team timing diagnostics, standings and guided debriefs. These need explicit backend/API work and should not be presented as cosmetic changes.

Validate the first two phases with eight teams, long names, unready/missing teams, a revoked connection, an expired admin session, a timeout draw with survivors, an aborted match and a replay perspective failure. Check the map and essential status at common laptop/projector sizes. No change to game physics is required for the core redesign.

## Implementation status — 12 September 2026

The first two implementation phases are implemented in the trainer console:

- Bounded split/full battlefield and replay canvases, independent roster/event scrolling, responsive header and replay controls, compact stable team rows, selected-team telemetry and ship highlight, distinct session colours with ship identifiers, display toggles and projector typography.
- Focused, labelled sign-in; authentication-gated polling; expired-session, connecting, disconnected and stale-data states; stale HTTP/WebSocket responses cannot restore a previous session. Navigation describes the viewed surface independently of the server's live state.
- Explicit match/team confirmations for abort and live forfeit, pending/error feedback, and retained views on failure. The lobby offers a cancellable five-second countdown.
- Capability-aware Analysis navigation; active rules, draft counts/change summaries, apply/discard gating, presets and rules import/export. Expected roster names and readiness blockers are sanitized administrator data.
- Server timing and match identifiers, named/filterable events, explicit forfeit/disconnect reasons, and recent-interval trainer metrics. The counters are not presented as team latency or late-command rates.
- Truthful hits-per-shot and draw wording; end reasons and forfeits in reports; damage bars, replay links and CSV/JSON exports. New replay end records distinguish outcomes while older ambiguous records remain unknown. Replay search/filtering, frame steps, previous/next event, in-view bookmarks and visible perspective errors support debriefs.

The third phase is now implemented: durable named training sessions and rounds, an expected-roster editor, preserved round reports, configurable training standings and CSV/JSON exports, per-team round-trip/response timing, a readiness/heartbeat preflight, persistent replay notes and labelled bookmarks, selected-team telemetry and synchronized sensor/ground-truth comparison. Training points are explicit and lock after the first round; they do not define official competition scoring. Aborted/interrupted rounds are excluded, forfeits earn zero and equal points share a place.

History and debriefs live beside the replays in `training-history.json`. A server restart preserves completed reports and marks unfinished rounds interrupted. Storage failures are visible and cannot silently acknowledge a session change. See the administrator session contract in `docs/PROTOCOL.md`.

The next extension, **presenter sound controls**, is implemented. The trainer can enable or immediately mute synthesized countdown and ship-elimination tones, set volume, choose cues and test each sound. Volume and cue choices persist locally; audio starts muted on reload and mutes on tab hiding or sign-out. Live cues exclude replay/background views, old events after reconnects and duplicate deaths; simultaneous deaths produce one cue. The countdown also cancels when the tab is hidden. All existing visual countdown, event and status information remains available with audio muted.

The public audience permission model and advanced version/heatmap analysis remain later features outside the review’s three implementation phases. No game physics or bot-facing protocol change was made.

Validation uses isolated local servers and synthetic/example teams. These checks are not measurements of the Amersfoort venue network. See `docs/ADMIN-FRONTEND-VALIDATION.md` for the completed checks and limitations.
