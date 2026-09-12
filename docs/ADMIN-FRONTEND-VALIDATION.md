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

Disconnect event timestamps reflect the tick when the administrator poll observes the disconnect; no bot latency is inferred from browser timing. Per-team latency/percentile instrumentation, actual venue network measurements and durable named training sessions remain future work. The review document records the remaining feature scope.
