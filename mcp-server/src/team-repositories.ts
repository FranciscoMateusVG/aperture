import { z } from "zod";

/**
 * Runtime repository registry contract (GLaDOS-only, via the same authenticated
 * control binary). Mirrors src-tauri/src/team_repository_catalog.rs exactly:
 * no paths, no actor, no team or activation inference. Disabling an entry only
 * blocks new create/approve admissions; stored team bindings never read this.
 */
const PROJECTS = ["project:aperture", "project:incluir", "project:beads-galaxy", "project:mempalace", "project:frame"] as const;
const MAX_REPOSITORIES = 128;
const MAX_DISPLAY_SCALARS = 80;
const MAX_DISPLAY_BYTES = 320;
// Same set the native validate_text rejects: control characters (Cc) plus bidi controls.
const UNSAFE_TEXT = /[\p{Cc}\u{061c}\u{200e}\u{200f}\u{202a}-\u{202e}\u{2066}-\u{2069}]/u;

export const repositoryKeySchema = z.string().min(1).max(64).regex(/^[a-z][a-z0-9._-]*$/);
export const sha256Schema = z.string().regex(/^[a-f0-9]{64}$/);
const displayName = z.string().refine(v =>
  v.trim().length > 0 && [...v].length <= MAX_DISPLAY_SCALARS && Buffer.byteLength(v) <= MAX_DISPLAY_BYTES && !UNSAFE_TEXT.test(v),
  { message: "display_name must be 1-80 scalars, at most 320 bytes, without control or bidi characters" });

export const saveRepositorySchema = z.object({
  project: z.enum(PROJECTS),
  repo: repositoryKeySchema,
  display_name: displayName,
  enabled: z.boolean(),
  expected_sha256: sha256Schema,
}).strict();
export type SaveRepositorySelectors = z.infer<typeof saveRepositorySchema>;

const entry = z.object({
  project: z.enum(PROJECTS),
  repo: repositoryKeySchema,
  display_name: displayName,
  enabled: z.boolean(),
  available: z.boolean(),
}).strict();
const registry = z.object({
  schema_version: z.literal(1),
  sha256: sha256Schema,
  // The native registry is never empty (seeds are materialized on first write), so an empty array is a malformed response.
  repositories: z.array(entry).min(1).max(MAX_REPOSITORIES),
}).strict();
export type RepositoryRegistryView = z.infer<typeof registry>;

const fail = (reason: string): never => { throw new Error(`E_CONTROL_FAILED: ${reason}; reread the registry before any retry`); };

/** Validate the real native envelope for one exact action; duplicates by (project, repo) are refused. */
export function parseRepositoryRegistry(value: unknown, action: "list_repositories" | "save_repository"): RepositoryRegistryView {
  const parsed = z.object({ action: z.literal(action), result: registry }).strict().safeParse(value);
  if (!parsed.success) return fail(`repository registry response did not match the ${action} contract`);
  const keys = new Set<string>();
  for (const r of parsed.data.result.repositories) {
    const key = `${r.project} ${r.repo}`;
    if (keys.has(key)) return fail("repository registry response contains a duplicate binding");
    keys.add(key);
  }
  return parsed.data.result;
}

/** A save is confirmed only when the returned registry echoes the exact saved entry. */
export function parseSavedRepository(value: unknown, input: SaveRepositorySelectors): RepositoryRegistryView {
  const view = parseRepositoryRegistry(value, "save_repository");
  const saved = view.repositories.find(r => r.project === input.project && r.repo === input.repo);
  if (!saved || saved.display_name !== input.display_name || saved.enabled !== input.enabled) {
    return fail("save response did not echo the requested repository entry");
  }
  return view;
}
