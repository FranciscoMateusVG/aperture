# Objectives Kanban + BEADS Browser

## Summary

Two new UI surfaces for the Aperture control panel:

1. **Objectives Kanban** — a six-column board that occupies the main area (where the terminal normally renders) when the "Objectives" navbar button is active. Provides the planning and approval workflow for work items.
2. **BEADS Panel** — a right-panel tab for browsing BEADS tasks and their artifacts. Auto-filters when an objective card is clicked on the Kanban.

## Data Model

### Objective

Stored in `~/.aperture/objectives.json` as a JSON array. No database — lightweight file-based persistence like spiderlings.

```typescript
interface Objective {
  id: string;              // nanoid or timestamp-based
  title: string;
  description: string;
  spec: string | null;     // Wheatley's spec in markdown
  status: "draft" | "speccing" | "ready" | "approved" | "in_progress" | "done";
  priority: number;        // 0-4 (0 = highest)
  task_ids: string[];      // linked BEADS task IDs
  created_at: string;      // ISO timestamp
  updated_at: string;      // ISO timestamp
}
```

### Relationship to BEADS

- An objective's `task_ids` array references BEADS task IDs (e.g., `["aperture-abc", "aperture-xyz"]`).
- BEADS tasks are the source of truth for execution status. Objectives are the planning layer.
- Artifacts live on BEADS tasks (files, PRs, notes, URLs) — the objective does not duplicate them.
- Objective progress is derived: count closed tasks / total tasks in `task_ids`.

## Layout

```
┌────────────────────────────────────────────────────────────────────────┐
│ NAVBAR   [Terminal] [Objectives]        Chat  WarRoom  Messages  BEADS  🕷️ │
├──────────────┬──────────────────────────────────────┬──────────────────┤
│              │                                      │                  │
│ SIDEBAR      │ MAIN AREA (toggles between):         │ RIGHT PANEL      │
│              │  • Terminal (default, xterm.js PTY)   │                  │
│ - Agents     │  • Objectives Kanban (6 columns)      │  • BEADS tab     │
│ - Sessions   │                                      │  • Chat          │
│              │                                      │  • War Room      │
│              │                                      │  • Messages      │
│              │                                      │  • Spiders       │
└──────────────┴──────────────────────────────────────┴──────────────────┘
```

The "Terminal" and "Objectives" buttons sit on the **left side** of the navbar (near the title). They switch the main area content. The right-panel tabs (Chat, WarRoom, Messages, BEADS, Spiders) remain on the right side of the navbar.

## Kanban Board

### Columns

Six columns, left to right:

| Column | Meaning |
|--------|---------|
| **Draft** | Raw idea. Title + description only. Has a "+" button to create new objectives and a "Spec it" button per card. |
| **Speccing** | Wheatley is actively writing the spec. Card shows a spinner/indicator. |
| **Ready** | Spec complete, awaiting operator review. Card has a checkbox for batch selection. |
| **Approved** | Operator has greenlit. GLaDOS can pick these up. |
| **In Progress** | GLaDOS is orchestrating — spiderlings spawned, BEADS tasks created. Shows progress bar. |
| **Done** | All tasks closed. Objective complete. |

### Card Design

Each card displays:
- **Title** (bold, truncated to 2 lines)
- **Priority badge** (color-coded: P0 red, P1 orange, P2 blue, P3 gray, P4 dim)
- **Progress bar** (only in "In Progress" and "Done" columns): `3/7 tasks` with a filled bar
- **Action button** (context-dependent):
  - Draft: "Spec it" button
  - Ready: checkbox for batch approve
  - In Progress: none (automated)

### Drag and Drop

Cards are draggable between columns using HTML5 Drag and Drop API. Dropping a card in a new column updates its status. Some transitions trigger side effects:

- **Draft → Speccing**: Not manual. Triggered only by "Spec it" button (sends message to Wheatley).
- **Any → Ready**: Manual drag or Wheatley updates it programmatically.
- **Ready → Approved**: Via batch "Approve Selected" button or individual drag.
- **Approved → In Progress**: Not manual. Triggered by GLaDOS when orchestration begins.
- **In Progress → Done**: Automatic when all linked BEADS tasks are closed.

### Batch Approve

A floating action bar appears at the bottom of the Kanban when one or more Ready cards are checked:

```
┌──────────────────────────────────────────────┐
│  ✓ 3 objectives selected    [Approve All]    │
└──────────────────────────────────────────────┘
```

Clicking "Approve All" moves them to Approved and sends a notification to GLaDOS.

### Create Objective

"+" button in the Draft column header opens an inline form:
- Title (required)
- Description (optional, textarea)
- Priority (dropdown, default P2)

Submitting creates the objective and renders the card in Draft.

## Speccing Workflow

1. Operator clicks "Spec it" on a Draft card.
2. Card moves to Speccing column.
3. System sends a message to Wheatley via the Aperture bus: `"New objective to spec: {title}. Description: {description}. Write a detailed spec and update the objective when done."`
4. Wheatley reads the objective, researches the codebase, writes a spec.
5. Wheatley calls `update_objective(id, { spec: "...", status: "ready" })` via MCP.
6. Card moves to Ready. Operator reviews the spec in the BEADS panel.

## Orchestration Flow

1. Operator batch-approves objectives (Ready → Approved).
2. GLaDOS detects approved objectives (via polling or message notification).
3. GLaDOS presents an execution plan to the operator via chat: "I'm planning to decompose these 3 objectives into N tasks. Here's the breakdown: ..."
4. Operator confirms (sends "go" via chat).
5. GLaDOS creates BEADS tasks for each objective, updates `task_ids`, spawns spiderlings.
6. Objectives move to In Progress.
7. As BEADS tasks close, the progress bar updates.
8. When all tasks for an objective are closed, it auto-moves to Done.

## BEADS Panel (Right Panel Tab)

### Layout

```
┌──────────────────────────┐
│ 🔍 Search tasks...       │
│ [Filter: All ▾]          │
├──────────────────────────┤
│ ● aperture-abc  P1       │
│   "Add auth to Fitt"     │
│   Status: open            │
│   ▶ 2 artifacts          │
├──────────────────────────┤
│ ● aperture-xyz  P2       │
│   "Fix login redirect"   │
│   Status: closed          │
│   ▶ 1 artifact           │
├──────────────────────────┤
│ ...                       │
└──────────────────────────┘
```

### Features

- **Search bar** — filters tasks by title or ID.
- **Filter dropdown** — "All", "Open", "Closed", or filter by objective (when clicked from Kanban).
- **Task rows** — show ID, title, priority, status. Clickable to expand.
- **Expanded task** — shows full description, notes, and a list of artifacts:
  - File artifacts: clickable path (opens in system file browser via `open` command)
  - PR artifacts: clickable URL
  - Note artifacts: inline text
  - URL artifacts: clickable link
- **Context filter** — when an objective card is clicked on the Kanban, the BEADS panel opens and filters to `task_ids` of that objective. A breadcrumb shows "Filtered by: {objective title}" with an "×" to clear.

### Polling

The BEADS panel polls `list_beads_tasks` every 3 seconds with hash-based diffing to avoid flicker (same pattern as AgentList).

## Backend Changes

### New Rust Module: `objectives.rs`

```rust
// CRUD operations on ~/.aperture/objectives.json

#[tauri::command]
pub fn list_objectives() -> Result<Vec<Objective>, String>

#[tauri::command]
pub fn create_objective(title: String, description: String, priority: u8) -> Result<Objective, String>

#[tauri::command]
pub fn update_objective(id: String, title: Option<String>, description: Option<String>, spec: Option<String>, status: Option<String>, priority: Option<u8>, task_ids: Option<Vec<String>>) -> Result<Objective, String>

#[tauri::command]
pub fn delete_objective(id: String) -> Result<(), String>
```

### New MCP Tools

Expose objective management to agents (Wheatley needs to update specs, GLaDOS needs to update task_ids and status):

- `list_objectives()` — returns all objectives
- `update_objective(id, fields)` — update any field (spec, status, task_ids)

Added to the MCP server's tool list, available to all agents.

### Enhanced BEADS Query

The existing `list_beads_tasks` Tauri command is sufficient. The BEADS panel handles filtering client-side based on the objective's `task_ids`.

## Frontend Components

### ObjectivesKanban.ts

- Renders 6 columns with cards.
- HTML5 drag-and-drop for card movement.
- "+" button for creating objectives.
- "Spec it" button triggers message to Wheatley.
- Batch checkbox + floating approve bar for Ready column.
- Polls `list_objectives()` every 3s with hash diffing.
- Clicking a card dispatches a custom event that the BEADS panel listens for.

### BeadsPanel.ts

- Replaces the current TasksPanel.
- Search bar and filter dropdown.
- Expandable task rows with artifact display.
- Listens for `objective-selected` custom events to auto-filter.
- File artifacts have an "Open" button that invokes a Tauri command to `open` the file path.

### main.ts Changes

- Add "Terminal" and "Objectives" buttons to left side of navbar.
- Toggle main area between terminal container and objectives container.
- Hide terminal (not destroy) when showing objectives, and vice versa.
- Register BEADS panel as a right-panel tab.

### index.html Changes

- Add `<div id="objectives-container" class="hidden"></div>` next to `#terminal-container`.
- Add `<div id="panel-beads" class="panel-view hidden"></div>` in the right panel.
- Add navbar buttons for Terminal/Objectives toggle and BEADS panel.

## CSS

Follow existing patterns:
- `.kanban` — flex row of columns, horizontal scroll if needed
- `.kanban__column` — flex column, min-width 180px
- `.kanban__column-header` — title + count badge
- `.kanban__card` — bg-card, border, radius, draggable cursor
- `.kanban__card--dragging` — opacity 0.5
- `.kanban__progress` — thin bar with filled portion
- `.kanban__approve-bar` — fixed bottom, bg-panel, slide-up animation
- `.beads-panel` — follows existing panel-view pattern
- `.beads-panel__task` — expandable row with artifact list

All using existing CSS variables (--bg-card, --border, --accent-*, etc.).

## Open File Artifact

New Tauri command to open files in the system file browser:

```rust
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

## Summary of Changes

| Area | Files | Change |
|------|-------|--------|
| Rust backend | `objectives.rs` (new) | CRUD for objectives JSON file |
| Rust backend | `lib.rs` | Register objective commands + open_file |
| MCP server | `index.ts`, `objectives.ts` (new) | Expose objective tools to agents |
| Frontend | `ObjectivesKanban.ts` (new) | Kanban board component |
| Frontend | `BeadsPanel.ts` (new) | BEADS browser replacing TasksPanel |
| Frontend | `main.ts` | Main area toggle, BEADS panel wiring |
| Frontend | `index.html` | New containers and navbar buttons |
| Frontend | `style.css` | Kanban and BEADS panel styles |
| Frontend | `tauri-commands.ts` | New command bindings |
| Frontend | `types.ts` | Objective interface |
