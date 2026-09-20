# V4 pre-implementation references

Owner: Vance · bead aperture-zfmd5 · source baseline 2703dc4.

Open `index.html` or an individual SVG. `manifest.json` hashes every vector and the token/component contracts. The images are **authored vector layouts**, not screenshots of implemented UI. Fixtures do not call Tauri, BEADS, tmux or a provider. Mockup data is illustrative, never a live status claim. Generic preset examples do not assert that backend presets already exist.

## Direction

Root approved preserving the actual navy/amber design system (message aperture-wisp-cvx2gl, 2026-09-20). Layout approval is still pending. Source: current `src/style.css` and spec §3 ASCII mockups; no external image assets were supplied or invented. Typography is system sans with monospaced identity metadata; six-pixel controls, restrained panel borders, amber only for primary intent. New text/control tokens improve contrast without restyling the existing roster.

- Library: reusable cards, edit/duplicate/blank, explicit snapshot semantics.
- Wizard: mission and acceptance before seats; derived names, one lead, configured harness/model/reasoning and fallback. Create means pending approval, not worker start.
- Grouped sessions: coordination and standing specialists preserved; project groups show configured/observed separately, generation, stale/unknown data honestly.
- Replace: prepare/stop/verify is separate from start. Check rows are read-only evidence, never client-checkable authorization. `ready` is backend authority, not a boolean inferred by the frontend. Closing cannot promise cancellation of completed stop/revoke effects.
- Archive: show item-level blockers, review/evidence/transfer acceptance; recheck before a separate archive action. No destructive cleanup implied.

## Component contract

`tokens.css` and `components.css` establish cards, buttons, input/select/textarea, tabs, dialogs, status/error badges and icon geometry before implementation. Minimum action targets 44×44; visible two-pixel focus; labels/errors associated in implemented DOM; dialog content scrolls with reachable sticky actions. Disabled actions retain an explanatory reason. Never communicate status with color alone.

## Evidence boundaries

1280×800 and 1024×768 are the **logical design artboard dimensions**, not measured runtime geometry. DPR and font rendering are not captured. No WKWebView/browser comparison, focus trap, installed binary, live team creation, replacement, archive, or provider operation was run. §7 final faithful-runner requirements remain pending, not waived. Model values in references are configuration fixtures, never inferred actual session models.
