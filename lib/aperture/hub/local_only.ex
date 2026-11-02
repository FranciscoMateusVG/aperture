defmodule Aperture.Hub.LocalOnly do
  @moduledoc """
  Only this machine may drive the hub: the Host must be loopback (blocks DNS
  rebinding) and a browser Origin, when present, must be this page (blocks
  other websites posting to localhost). Non-browser clients send no Origin.
  """
  @behaviour Plug
  import Plug.Conn

  @impl true
  def init(opts), do: opts

  @impl true
  def call(conn, _opts) do
    port = Application.fetch_env!(:aperture, :port)
    allowed = ["http://127.0.0.1:#{port}", "http://localhost:#{port}"]

    origin_ok =
      case get_req_header(conn, "origin") do
        [] -> true
        [origin] -> origin in allowed
        _ -> false
      end

    if conn.host in ["127.0.0.1", "localhost"] and origin_ok do
      conn
    else
      conn |> send_resp(403, "forbidden") |> halt()
    end
  end
end
