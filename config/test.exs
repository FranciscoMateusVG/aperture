import Config

config :aperture,
  # APERTURE_TEST_PORT lets two checkouts run the suite at once
  port: String.to_integer(System.get_env("APERTURE_TEST_PORT", "4599")),
  db_path: Path.join(System.tmp_dir!(), "aperture-test-#{System.os_time()}.db"),
  uploads_dir: Path.join(System.tmp_dir!(), "aperture-test-uploads-#{System.os_time()}"),
  # short: unix socket paths live here and macOS caps them at 104 bytes
  run_dir: Path.join(System.tmp_dir!(), "apr#{rem(System.os_time(), 1_000_000)}"),
  codex_retry_ms: 50,
  agents_dir: Path.join(System.tmp_dir!(), "aperture-test-agents-#{System.os_time()}"),
  agent_overrides:
    Path.join(System.tmp_dir!(), "aperture-test-overrides-#{System.os_time()}.json"),
  tmux_socket: "aperture-test-#{System.os_time()}",
  tmux_session: "aperture-test",
  claude_bin: Path.expand("../test/support/bin/fake-claude", __DIR__),
  codex_bin: Path.expand("../test/support/bin/fake-codex", __DIR__),
  codex_config: Path.join(System.tmp_dir!(), "aperture-test-codex-config.toml"),
  codex_wait_ms: 5_000,
  launch_env: %{
    "FAKE_LOG_DIR" => Path.join(System.tmp_dir!(), "aperture-test-fake-#{System.os_time()}")
  },
  codex_backoff_ms: [20, 50, 100, 200],
  beads_dir: Path.join(System.tmp_dir!(), "aperture-test-beads"),
  memory_index:
    Path.join(System.tmp_dir!(), "aperture-test-memory-index-#{System.os_time()}.json"),
  claude_json: Path.join(System.tmp_dir!(), "aperture-test-claude-#{System.os_time()}.json"),
  teams_path: Path.join(System.tmp_dir!(), "aperture-test-teams-#{System.os_time()}.json"),
  worktrees_dir: Path.join(System.tmp_dir!(), "aperture-test-wt-#{System.os_time()}"),
  bd_bin: Path.expand("../test/support/bin/fake-bd", __DIR__),
  # the page's BEADS poll would add calls to fake-bd's log mid-test
  team_tasks_every_ms: nil

config :logger, level: :warning
