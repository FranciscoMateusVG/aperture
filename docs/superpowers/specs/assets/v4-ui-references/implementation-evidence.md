# P1 UI implementation evidence (work in progress)

Source base `origin/master` 2703dc4. Task aperture-zfmd5. Layout approved by GLaDOS in aperture-wisp-ivi8hg; dark direction in aperture-wisp-cvx2gl. Backend contract Rex v3 (`/tmp/aperture-4yk4o-p1-contract-v3.md`, digest prefix 0abf1b00).

## Implemented source

- TypeScript DOM components, no React or new package/framework. Existing AgentList and controls retained, coordination/standing grouped before project teams.
- Preset library and native-dialog editor: create from preset, edit with backend SHA CAS, duplicate/blank with new ID, detached submission snapshots, derived repeated-seat names, one explicit lead, current backend catalog execution tuples/roles.
- Exact mutation-response identity and configured-snapshot consistency are checked. Fixed error copy; pending registration never claims notification/worker boot. Backend errors and malformed success remain errors. No auto-retry.
- Model configuration and observations rendered separately; missing owner, turn, context and checkpoint remain unknown/unavailable.
- P1 wrappers invoke only frozen catalog/preset/list/create/cancel commands; no activation endpoint, actor/grants/source claims or caller paths.
- Replacement/archive dialogs deliberately show unavailable evidence and disabled actions until P3 integration. These are not completed capabilities.

## Source tests

`node --test tests/seat-name-ui.test.mjs tests/team-draft.test.mjs tests/team-contract.test.mjs tests/team-editor.test.mjs tests/teams-area.test.mjs` → 56/56 PASS.

`npm run build` → TypeScript and Vite PASS. No install/setup/restart or live team operation executed.

Tests use existing node:test + Vite SSR with a limited FakeElement handler-contract fixture. They execute actual form handlers for invalid/edit/paste/delete values, exact submission, synchronous duplicate-submit locking, pending error/retry, Escape cancellation veto while in flight, source focus-restoration calls, explicit lead reselection, detached presets, CAS, catalog bounds, malformed DTO rejection and escaped content. The fake DOM does **not** model browser focus, tab order, geometry, native dialog trapping, user-visible selection/paste/caret, or fonts.

## Outstanding gates

- Compare wrappers and DTO mirror against immutable actual Rex implementation before joint head.
- Integrate Peppy replacement/archive commands and their real evidence, generation/permit gates and failure states.
- Independent Izzy source review on immutable head.
- Faithful WKWebView geometry/focus/pixel comparison at 1280×800 and 1024×768: **NOT_RUN**, not waived.
- Scoped installed primary journey/runtime/model/provider observations: **NOT_RUN**; source is not installed adoption.
