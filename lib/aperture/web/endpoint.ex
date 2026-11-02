defmodule Aperture.Web.Endpoint do
  @moduledoc """
  Phoenix endpoint for the launcher page. It has no server of its own:
  `Aperture.Web.Front`, on the hub's Bandit, hands it `/`, `/live` and
  `/assets`.
  """
  use Phoenix.Endpoint, otp_app: :aperture

  @session_options [
    store: :cookie,
    key: "_aperture_key",
    signing_salt: "aperture-ui",
    same_site: "Strict"
  ]

  def session_options, do: @session_options

  socket("/live", Phoenix.LiveView.Socket,
    websocket: [connect_info: [session: @session_options]],
    longpoll: false
  )

  # Phoenix's and LiveView's client JS straight from the deps: no JS toolchain.
  plug(Plug.Static,
    at: "/assets/vendor",
    from: {:phoenix, "priv/static"},
    only: ~w(phoenix.min.js)
  )

  plug(Plug.Static,
    at: "/assets/vendor",
    from: {:phoenix_live_view, "priv/static"},
    only: ~w(phoenix_live_view.min.js)
  )

  plug(Plug.Static, at: "/assets", from: {:aperture, "priv/static"}, only: ~w(app.css app.js))

  # the installable app (PWA): the service worker must live at the root to
  # control "/"
  plug(Plug.Static,
    at: "/",
    from: {:aperture, "priv/static"},
    only: ~w(manifest.webmanifest sw.js offline.html icons),
    headers: %{"cache-control" => "no-cache"}
  )

  plug(Plug.Parsers,
    parsers: [:urlencoded, :multipart],
    pass: ["*/*"]
  )

  plug(Plug.Session, @session_options)
  plug(Aperture.Web.Router)
end
