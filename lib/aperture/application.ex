defmodule Aperture.Application do
  @moduledoc false
  use Application

  @impl true
  def start(_type, _args) do
    put_secret_key_base()

    children = [
      {Registry, keys: :unique, name: Aperture.Connections},
      {Phoenix.PubSub, name: Aperture.PubSub},
      {Aperture.Store, path: Application.fetch_env!(:aperture, :db_path)},
      {Registry, keys: :unique, name: Aperture.Codex.Bridges},
      {DynamicSupervisor, name: Aperture.Codex.Supervisor, strategy: :one_for_one},
      Aperture.Presence,
      Aperture.Launcher,
      Aperture.Teams,
      Aperture.Beads.Queue,
      Aperture.Web.TeamTasks,
      Aperture.Web.Endpoint,
      Supervisor.child_spec(
        {Bandit,
         plug: Aperture.Web.Front,
         ip: {127, 0, 0, 1},
         port: Application.fetch_env!(:aperture, :port),
         startup_log: false},
        id: :endpoint
      )
    ]

    Supervisor.start_link(children, strategy: :one_for_one, name: Aperture.Supervisor)
  end

  # The page's session cookie is signed with this; a fresh one per boot keeps
  # any secret out of the repo (the only cost: open pages reload on restart).
  defp put_secret_key_base do
    config = Application.get_env(:aperture, Aperture.Web.Endpoint, [])

    unless config[:secret_key_base] do
      secret = Base.encode64(:crypto.strong_rand_bytes(48))
      Application.put_env(:aperture, Aperture.Web.Endpoint, [secret_key_base: secret] ++ config)
    end
  end
end
