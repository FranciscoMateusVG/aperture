defmodule Aperture.AgentFixtures do
  @moduledoc "Writes agent definitions into the test agents_dir."

  def define(name, attrs \\ %{}) do
    dir = Application.fetch_env!(:aperture, :agents_dir)
    File.mkdir_p!(dir)

    defn =
      Map.merge(
        %{
          "name" => name,
          "display" => String.capitalize(name),
          "role" => "test agent",
          "prompt" => Path.join(dir, "#{name}.md"),
          "harness" => "claude",
          "model" => "opus"
        },
        attrs
      )

    File.write!(Path.join(dir, "#{name}.md"), "# #{name}\nTest prompt for #{name}.\n")
    File.write!(Path.join(dir, "#{name}.json"), JSON.encode!(defn))
    name
  end

  def unique(prefix), do: "#{prefix}#{System.unique_integer([:positive])}"
end
