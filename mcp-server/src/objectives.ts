import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

export interface Objective {
  id: string;
  title: string;
  description: string;
  spec: string | null;
  status: string;
  priority: number;
  task_ids: string[];
  created_at: string;
  updated_at: string;
}

function objectivesPath(): string {
  return join(homedir(), ".aperture", "objectives.json");
}

export function listObjectives(): Objective[] {
  try {
    const data = readFileSync(objectivesPath(), "utf-8");
    return JSON.parse(data);
  } catch {
    return [];
  }
}

function writeObjectives(objectives: Objective[]): void {
  writeFileSync(objectivesPath(), JSON.stringify(objectives, null, 2));
}

export function updateObjectiveFile(
  id: string,
  fields: {
    title?: string;
    description?: string;
    spec?: string;
    status?: string;
    priority?: number;
    task_ids?: string[];
  }
): Objective {
  const objectives = listObjectives();
  const obj = objectives.find((o) => o.id === id);
  if (!obj) throw new Error(`Objective '${id}' not found`);

  if (fields.title !== undefined) obj.title = fields.title;
  if (fields.description !== undefined) obj.description = fields.description;
  if (fields.spec !== undefined) obj.spec = fields.spec;
  if (fields.status !== undefined) obj.status = fields.status;
  if (fields.priority !== undefined) obj.priority = fields.priority;
  if (fields.task_ids !== undefined) obj.task_ids = fields.task_ids;
  obj.updated_at = String(Date.now());

  writeObjectives(objectives);
  return obj;
}
