defmodule Aperture.MixProject do
  use Mix.Project

  def project do
    [
      app: :aperture,
      version: "4.0.0-dev",
      elixir: "~> 1.18",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  def application do
    [extra_applications: [:logger], mod: {Aperture.Application, []}]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  defp deps do
    [
      {:bandit, "~> 1.6"},
      {:websock_adapter, "~> 0.5"},
      {:exqlite, "~> 0.27"},
      {:mint_web_socket, "~> 1.0"},
      {:phoenix_pubsub, "~> 2.1"},
      {:phoenix, "~> 1.8"},
      {:phoenix_live_view, "~> 1.2"},
      {:phoenix_html, "~> 4.3"},
      {:lazy_html, "~> 0.1.13", only: :test}
    ]
  end
end
