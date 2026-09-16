# Admin frontend implementation validation

Validated 12 September 2026 using isolated local servers, eight Python example bots and synthetic participant credentials. The production service and game physics were unchanged.

## Automated checks

- `npm run check`: no Svelte/TypeScript errors or warnings.
- `npm test`: 56 tests pass, including signed-out polling, protected 401 expiry, stale HTTP/WebSocket responses after logout, draft start blocking, eight distinct team colours and readable events.
- `npm run build`: regenerated `spectator/dist`, followed by a Rust server build so the embedded frontend is current.
- `cargo test`: the complete suite passes with local socket access. The sandboxed attempt cannot bind the integration-test ports; this is an environment restriction, not a product failure.
- Additional targeted Rust regressions pass for retained forfeited hulls/reasons, timeout draws with survivors and a 1.5 hit-events-per-shot ratio, and replay metadata that distinguishes ambiguous historical records from explicit draws and aborts.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check` pass.

## Browser checks

Chrome was driven against a local tournament-mode server with a 900 × 500 map and eight teams, including a long team name. A loopback-only fault proxy was used for failures; no production requests were made.

| Check | Result |
|---|---|
| Sign-in | Focused labelled form; no protected parameter-loading screen before authentication. A protected 401 returns to “Session expired”. |
| Draft rules | Editing hull HP creates a visible change summary. Navigating away preserves the draft and disables start. Applying updates the active hash/preset and teams acknowledge it. |
| Countdown | Start offers a five-second countdown; cancellation leaves the match in the lobby. |
| Laptop layout | At 1,366 × 768 the split canvas is 952 × 521, bounded inside the viewport. Expanded canvas is 1,334 × 521. Document remains 1,366 × 768. |
| Tablet layout | At 768 × 1,024 the map and independent roster fit; document remains 768 × 1,024. |
| Small effective viewport | At 683 × 384, matching the CSS viewport available at 200% zoom on a 1,366 × 768 display, map, clock and independently scrollable roster/events remain visible. Replay map remains visible with a perspective error. This verifies the effective viewport, not every browser/OS zoom combination. |
| Narrow lobby | At 390 × 844, the lobby uses internal scrolling with no document overflow. |
| Selection/layout | Selecting a team expands telemetry; split/expanded toggles preserve map containment. |
| Failed abort/forfeit | Injected HTTP 503 responses show inline errors and keep the match and confirmations available. |
| Revoked participant | Revoking an example credential shows “Forfeited”, “credentials revoked”, and a named disconnect event while the hull remains. The report preserves the forfeit. |
| Failed reset | With an ended-room fixture and a failed reset response, the report stays open with an inline error. |
| Result/replay | Successful abort produces “Match aborted” and “operator abort”; Watch replay opens paused playback. The library labels the record “aborted”. |
| Replay failure | Failed perspective request is visible in the viewer, explicitly explaining the return to overall ground truth. Frame/event stepping and an in-view bookmark work. |
| Missing team | Re-enabling the absent example participant shows “Not connected” in the expected roster. |
| API sanitization | Room response includes capabilities/timing/roster names and contains no participant credentials. Tournament mode hides Analysis. |

Primary text colours were checked against the panel background: normal text, secondary text, accent, success and error colours exceed 4.5:1. Keyboard focus styles and labelled controls are present. This is not a complete assistive-technology or contrast audit of every legacy analysis control.

Disconnect event timestamps reflect the tick when the administrator poll observes the disconnect; no bot latency is inferred from browser timing. The third-phase additions below implement per-team instrumentation and durable sessions; actual venue network measurements remain an on-site task. The review document records the remaining feature scope.

## Third phase — saved sessions, timing and debriefs

Implemented and validated on 12 September 2026 with an isolated local tournament server and eight Python example bots, including a long team name. The session history and credentials used for these checks are synthetic and stored under `/tmp/admin-phase3-session`.

- Automated checks: 61 frontend tests across 10 files; 222 Rust tests across the unit/integration suites; TypeScript/Svelte check with zero errors or warnings; Clippy with warnings denied; formatting, frontend production build, server rebuild and `git diff --check` passed.
- Added coverage for bounded timing percentiles/reset, exact admission versus late/duplicate/wrong-tick diagnostics, atomic history failure handling, preservation of corrupt history, restart interruption, persistent debriefs, scoring locks, history revision conflicts, authenticated training routes, named round/report/timing persistence, team identity across reconnects, excluded abort/interruption results, forfeits, tied standings, configured points, CSV formula neutralization, stale authentication responses and failed history saves.
- Browser: created `Amersfoort practice`, kept the eight expected identities, named its first round `Sensor warm-up`, completed the 15-second readiness/heartbeat check (all eight received heartbeats), started the named round, inspected live response/RTT statistics, and aborted it with its report retained. The report showed bounded response samples and separate late/wrong-tick/missed-window counters. These are loopback measurements, not venue network evidence.
- Saved a labelled bookmark at a team hit and discussion notes; synchronized the selected team's sensor view with overall ground truth; inspected selected-team controls and contacts. Restarted the real server, signed in again, recovered the active session and round history, opened its historical report, and confirmed the saved bookmark and notes remained. The historical report remained selected through the latest-report polling cycle.
- Browser checks found and fixed a comparison-canvas sizing bug caused by the old stage alignment. At 1366 × 768 with the guide open, both canvases measured 483 × 449 CSS pixels. At 768 × 1024 with the guide closed, both measured 364 × 625. At the 683 × 384 CSS viewport equivalent of a 200% laptop view, the comparison stacked into two 667 × 69.5 canvases, with a separately scrollable guide overlay and reachable close control. The document remained the requested viewport size in each check. This is viewport testing, not an OS/browser zoom audit; comparison maps are necessarily small in the shortest viewport and the single-view toggle remains available. The 390 × 844 check also caught and fixed an inherited grid layout on Sessions: the final page has no horizontal overflow, while the 763-pixel standings table scrolls inside a 325-pixel container.

Saved-session persistence does not resume a running simulation after a crash, recover an unwritten report from a partial replay, or persist unsaved editor drafts. An unfinished recorded round becomes `interrupted`; storage failures stay visible and pending completed reports can be exported before shutdown. One live server must own each history directory. Official competition rules, a separately authorized audience page, optional audio cues and advanced version/heatmap analysis remain outside this phase.

## Presenter extension — optional sound cues

Implemented and validated on 12 September 2026 against an isolated local tournament server with eight example teams (`/tmp/admin-phase4-session`).

- Added a visible Enable/Mute control and a keyboard-accessible sound settings dialog with volume, separate countdown/elimination choices and test buttons. Short tones use the browser's Web Audio API; no assets, dependencies, server changes or bot protocol changes are required. Existing visual countdowns and events remain available while muted.
- Audio requires an explicit enable gesture on every page load. Only volume and cue choices are stored in the browser. Sign-out or tab hiding mutes audio, stops scheduled notes and invalidates a pending audio setup. Hiding the tab also cancels the local start countdown. Browser audio failures are visible and do not block match controls.
- Automatic elimination cues follow consecutive fresh live frames. The first frame after enabling, returning to Live, reconnecting, a tick rewind or a match change establishes a silent baseline. Repeated deaths and missed/stale frames do not cause catch-up sounds; simultaneous deaths produce one cue. Forfeits are not inferred from hull presence. Replay and background views have no automatic elimination sounds.
- Automated checks: all 74 frontend tests across 12 files pass, including audio setup/mute races, missing or interrupted Web Audio, bounded overlapping voices, cleanup/volume, cue deduplication, stale frames, replay navigation, authentication and visibility transitions. Svelte/TypeScript reports zero errors or warnings. Production frontend build, embedded server rebuild, Rust formatting check, Clippy with warnings denied and `git diff --check` pass. Rust implementation is unchanged in this extension; the preceding phase's full server suite was not rerun.
- Browser: confirmed initially muted state, enabled audio, successfully scheduled both test tones, used the immediate mute control, changed volume and cue selection, reloaded and recovered preferences while remaining muted. Cancelled a start countdown and started an eight-team match. Signed out with audio enabled. Escape closes the native dialog and restores focus to Sound settings.
- Layout: document dimensions stayed within 1366 × 768, 768 × 1024, 683 × 384 and 390 × 844 CSS viewports. Live canvas content measured 950 × 519, 428 × 765 and 343 × 125 at the first three sizes. The sound dialog is internally scrollable at 683 × 384; its close button is reachable and Escape remains available. The 683 × 384 check represents the effective viewport of a laptop at 200% zoom, not every OS/browser zoom configuration.

Browser verification confirms audio setup and scheduling, not acoustic output from a physical speaker or venue loudness. Tab visibility behavior is covered by automated event tests; creating another automation tab did not switch the foreground tab in this browser. Test the chosen volume on the presenting device before the session.

## Full HD battlefield follow-up

On 12 September 2026, the admin tunnel at `127.0.0.1:8787` was confirmed to serve the older 14,853-byte stylesheet without the trainer app shell, bounded battle stage or viewport-height rules. The running VPS frontend therefore predates the sizing fixes described above. Updating a local bundle does not update the embedded frontend in that deployed server.

The current stylesheet now has one canvas sizing rule, with the obsolete square aspect ratio removed. The app shell has an explicit bounded height, the battle grid receives exactly the remaining space below navigation, and canvas layout is isolated from its container size. Long round names cannot widen the header. The renderer preserves the full map aspect ratio through letterboxing and keeps a 5% inset around the world so boundary lines and nearby ship markers stay clear of the canvas edge; team and event lists scroll within their own regions.

Verified with eight example teams on a 700 × 700 map matching the VPS. In each case, `scrollWidth`/`scrollHeight` matched the available size for the battle stage, battle main, app shell, app root, body and document. The map's complete world rectangle stayed within the canvas and viewport, rather than being hidden beyond a clipped parent.

| CSS viewport | Mode | Canvas border box | Page or battlefield overflow |
|---|---|---|---|
| 1920 × 1080 | Roster | 1506 × 833 | None |
| 1920 × 1080 | Expanded map | 1888 × 833 | None |
| 1920 × 1080 | Projector, roster | 1466 × 825 | None |
| 1920 × 1080 | Projector, expanded map | 1888 × 825 | None |
| 1920 × 900 | Projector, roster | 1466 × 645 | None |
| 1366 × 768 | Projector, roster | 912 × 513 | None |

Expanding a team's telemetry left the canvas and all its ancestor dimensions unchanged. The 1920 × 900 check also allows room for browser chrome on a Full HD monitor.

Release candidate `r20260912-1341` is built from committed server/frontend code at `3bf5d5b` plus this CSS and canvas-fit correction, isolating unrelated SDK migration and protocol edits in progress. Checks passed: 83 frontend tests, 222 Rust tests, 85 Python tests from the release snapshot, Svelte/TypeScript, formatting and Clippy. Dependency and final-image scans reported zero known vulnerabilities. The container health probe passed, and its embedded CSS/JavaScript bytes exactly match the browser-tested build. The release bundle is under `release-artifacts/r20260912-1341/`. After explicit approval, production activation completed on 12 September 2026 at 18:36 UTC. The container is healthy with zero restarts or startup errors; the existing administrator tunnel serves the exact verified CSS and JavaScript hashes, and administrator login succeeds. All six previously connected bots reconnected to the lobby. The previous release, `r20260912-2`, is retained for rollback. Deployment evidence is saved in `deployment-verification.json` in the release bundle.

Nine canvas-fit regressions cover Full HD roster/expanded/projector sizes, browser chrome, square/wide/tall maps, tiny resize dimensions, aspect ratio, centering and HiDPI scaling. With the final inset, the Full HD roster view displays the entire square world in a 749.7 × 749.7 region starting at (394.15, 224.65), inside the 1506 × 833 canvas. The screenshot is saved in the workspace’s `output/battle-fullhd-roster.jpg`.
