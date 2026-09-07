# Aperture agent instructions

Canonical shared rules: `.claude/skills/constitution/SKILL.md`. Detailed procedures live in `beads`, `communicate`, `worktree-discipline`, `specialist-delegation`, and `orchestrator-core`. Do not infer permission from historical examples.

## Task ownership and delivery

- GLaDOS discovers/assigns work. Specialists fetch only their assigned bead and await scoped dispatch; no ready/list/search sweeps or self-claims of unassigned work.
- Only GLaDOS files beads, only after explicit operator acknowledgment. Every bead has exactly one approved project label. Findings within current scope belong in its concise notes; other work is proposed, not automatically filed.
- Claim the existing bead before work. Edit in a per-task worktree from the actual canonical remote branch; never use the shared main checkout as an editing surface.
- A bounded repair discovered during assigned work may be fixed by its finder after the current owner explicitly agrees to the file handoff. Follow `communicate` §10: one pen per file set, own worktree, focused regression, independent diff review. This is not permission to reassign the whole task or expand into architecture/security/infra work.
- Store deliverables as artifacts and open a PR; ordinary task closure is PR-open, unless acceptance explicitly requires a later QA verdict. Do not claim unfinished operations complete.
- Use `bd update <id> --append-notes` for short progress notes. Put long reports in artifacts, not growing notes blobs. No blanket issue filing, stash clearing, branch deletion, force push, or endless failed-push retries at session end.

## Evidence and review

Use the smallest sufficient test layer. Ordinary UI/input logic uses unit/component tests; E2E is reserved for agreed primary user journeys and consequential reproduced failures that lower tests cannot faithfully cover. Follow `verify-user-path` for a scoped browser check, not an every-control campaign. One reviewer checks the exact diff; no self-approval after a finder repair. Required release sign-off remains explicit; operator risk acceptance is recorded as such, never rewritten as a passing test.

## Coordination and deployment

One execution owner per bounded change. Message only a next actor, decision owner, or materially affected colleague; no routine all-agent FYIs or acknowledgments of acknowledgments. All such messages use BEADS.

Before implementation, summarize the actual stack, rendering/runtime constraints, deployment branch and existing tooling. Routine approved code delivery uses the configured merge-to-Dokploy path; initial provisioning, environment/schema changes and exceptional cutovers are separate. Do not commission a custom broker, per-service script or replacement deployment framework when supported native tools/configuration can do the job. If access is missing, name the missing native setup rather than inventing a transport.

## Lanes

GLaDOS: orchestration and shared instructions. Wheatley: planning/research. Peppy: infrastructure/deployment. Izzy: scoped QA and bounded owner-consented repairs. Vance: frontend/design/performance. Rex: backend/APIs. Scout: mobile. Cipher: security. Implementers write their docs; `.claude/skills/team` is the complete roster.

## How instructions load

Canonical sources are `prompts/<agent>.md`, `.claude/skills/<skill>/SKILL.md`, and `agents/<agent>/{skills.txt,resident.txt}`. `just setup` links them into `~/.claude/aperture/<agent>/`; `src-tauri/src/agents.rs` assembles resident skill bodies on both harness paths, while non-resident skills remain discoverable. Without resident.txt, all assigned skills are injected. See `scripts/skills-matrix.sh` for the current mapping.

Changing a PR does not update running sessions. Report source/merge/setup/session adoption separately. Do not relaunch the fleet or run setup solely to claim new rules are active.
