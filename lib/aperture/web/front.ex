defmodule Aperture.Web.Front do
  @moduledoc """
  What Bandit serves on the hub's port. Local requests only
  (`Aperture.Hub.LocalOnly`), then:

    * `GET /`, `/live/*` (LiveView socket), `/assets/*` and the PWA files
      (`/manifest.webmanifest`, `/sw.js`, `/offline.html`, `/icons/*`) go to
      the LiveView page, `Aperture.Web.Endpoint`;
    * everything else goes to `Aperture.Hub.Endpoint` unchanged: the agents'
      `/inbox` socket, `/messages`, the MCP routes, presence, teams, …
  """
  use Plug.Builder

  @pwa_files ~w(manifest.webmanifest sw.js offline.html)

  plug(Aperture.Hub.LocalOnly)
  plug(:dispatch)

  @doc false
  def dispatch(conn, _opts) do
    case conn.path_info do
      [] -> Aperture.Web.Endpoint.call(conn, [])
      ["live" | _] -> Aperture.Web.Endpoint.call(conn, [])
      ["assets" | _] -> Aperture.Web.Endpoint.call(conn, [])
      ["icons" | _] -> Aperture.Web.Endpoint.call(conn, [])
      [file] when file in @pwa_files -> Aperture.Web.Endpoint.call(conn, [])
      _ -> Aperture.Hub.Endpoint.call(conn, Aperture.Hub.Endpoint.init([]))
    end
  end
end
