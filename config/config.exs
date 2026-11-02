import Config

config :aperture,
  port: 4518,
  db_path: Path.expand("../data/aperture.db", __DIR__),
  # images the operator attaches in the chat (served only by the local hub)
  uploads_dir: Path.expand("../data/uploads", __DIR__),
  run_dir: Path.expand("../data/run", __DIR__),
  root: Path.expand("..", __DIR__),
  agents_dir: Path.expand("../agents", __DIR__),
  agent_overrides: Path.expand("../data/agents.json", __DIR__),
  # tmux session the agents run in; nil socket = your default tmux server
  tmux_session: "aperture-v4",
  tmux_socket: nil,
  claude_bin: "claude",
  codex_bin: "codex",
  # Codex agents share ~/.codex; every [mcp_servers.X] defined there is
  # disabled for them so they only get the aperture tools.
  codex_config: Path.expand("~/.codex/config.toml"),
  codex_wait_ms: 30_000,
  # BEADS: only the hub runs bd (Dolt credentials stay in the hub's env).
  # v3's location, so existing tasks carry over.
  beads_dir: Path.expand("~/.aperture/.beads"),
  bd_bin: System.find_executable("bd") || "/opt/homebrew/bin/bd",
  # v3's recall memory (BEADS memory bank, redacted + indexed by v3), read-only
  memory_index: Path.expand("~/.aperture/run/memory-index.json"),
  # models offered per harness in the launcher (any other id still works
  # via "Other…"; an agent's current model is always listed)
  models: %{
    "claude" => ["opus", "opus[1m]", "sonnet", "sonnet[1m]", "haiku", "opusplan"],
    "codex" => ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.3-codex"]
  },
  # teams: who may create them, how big, where they live; coders' worktrees
  # go under worktrees_dir. All agents use the launcher's unconditional
  # permission bypass, regardless of harness, model or team membership.
  team_leads: ["glados"],
  team_max_members: 6,
  teams_path: Path.expand("../data/teams.json", __DIR__),
  worktrees_dir: Path.expand("../data/worktrees", __DIR__),
  # Claude Code's own config; team members' folders are marked trusted here
  # before they launch (Claude asks once per new folder otherwise)
  claude_json:
    Path.join(System.get_env("CLAUDE_CONFIG_DIR") || System.user_home!(), ".claude.json"),
  # extra environment for launched agents (tests use it)
  launch_env: %{}

# The launcher page (LiveView). Served by the hub's own Bandit through
# Aperture.Web.Front, so no server here. secret_key_base is generated at boot
# (Aperture.Application): sessions only live as long as the hub.
config :aperture, Aperture.Web.Endpoint,
  url: [host: "127.0.0.1"],
  server: false,
  pubsub_server: Aperture.PubSub,
  live_view: [signing_salt: "aperture-lv"],
  check_origin: ["//127.0.0.1", "//localhost"],
  render_errors: [formats: [html: Aperture.Web.ErrorHTML], layout: false]

config :phoenix, :json_library, JSON

import_config "#{config_env()}.exs"
