export interface AgentDef {
  name: string;
  model: string;
  role: string;
  prompt_file: string;
  tmux_window_id: string | null;
  status: string; // "stopped" | "running" | "error"
}

export interface WindowInfo {
  window_id: string;
  name: string;
  command: string;
}

export interface ChatMessage {
  from: string;
  to: string;
  content: string;
  timestamp: string;
}

export interface AgentMessage {
  id: number;
  from_agent: string;
  to_agent: string;
  content: string;
  timestamp: string;
  read: number;
}

export interface WarRoomState {
  id: string;
  topic: string;
  participants: string[];
  current_turn: number;
  current_agent: string;
  round: number;
  status: string; // "active" | "concluded"
  created_at: string;
  conclude_votes: string[];
}

export interface TranscriptEntry {
  role: string;
  content: string;
  timestamp: string;
  round?: number;
}

export interface SpiderlingDef {
  name: string;
  task_id: string;
  tmux_window_id: string | null;
  worktree_path: string;
  worktree_branch: string;
  requested_by: string;
  status: string;
  spawned_at: string;
}

export interface Objective {
  id: string;
  title: string;
  description: string;
  spec: string | null;
  status: "draft" | "speccing" | "ready" | "approved" | "in_progress" | "done";
  priority: number;
  task_ids: string[];
  created_at: string;
  updated_at: string;
}
