defmodule Aperture.Web.Router do
  use Phoenix.Router
  import Phoenix.LiveView.Router
  import Plug.Conn

  pipeline :browser do
    plug(:accepts, ["html"])
    plug(:fetch_session)
    plug(:protect_from_forgery)
    plug(:put_secure_browser_headers)
    plug(:put_root_layout, html: {Aperture.Web.Layouts, :root})
  end

  scope "/" do
    pipe_through(:browser)
    live("/", Aperture.Web.DashboardLive, :index)
  end
end
