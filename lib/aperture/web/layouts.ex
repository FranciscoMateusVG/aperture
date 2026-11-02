defmodule Aperture.Web.Layouts do
  @moduledoc false
  use Phoenix.Component

  def root(assigns) do
    ~H"""
    <!doctype html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="csrf-token" content={Phoenix.Controller.get_csrf_token()} />
        <.live_title default="Aperture">{assigns[:page_title]}</.live_title>
        <meta name="theme-color" content="#0f1115" />
        <link rel="icon" href="/icons/aperture.svg" type="image/svg+xml" />
        <link rel="apple-touch-icon" href="/icons/apple-touch-icon.png" />
        <link rel="manifest" href="/manifest.webmanifest" />
        <link rel="stylesheet" href="/assets/app.css" />
        <script defer src="/assets/vendor/phoenix.min.js"></script>
        <script defer src="/assets/vendor/phoenix_live_view.min.js"></script>
        <script defer src="/assets/app.js"></script>
      </head>
      <body>
        {@inner_content}
      </body>
    </html>
    """
  end
end
