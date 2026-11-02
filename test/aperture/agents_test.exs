defmodule Aperture.AgentsTest do
  use ExUnit.Case, async: false

  alias Aperture.{AgentFixtures, Agents}

  test "definitions give each agent its default harness and model" do
    name =
      AgentFixtures.define(AgentFixtures.unique("def"), %{
        "harness" => "codex",
        "model" => "gpt-6-astra"
      })

    assert %Agents.Agent{harness: "codex", model: "gpt-6-astra", role: "test agent"} =
             Agents.get(name)
  end

  test "the operator's choice overrides the default and is kept" do
    name = AgentFixtures.define(AgentFixtures.unique("ovr"))

    assert {:ok, %{harness: "codex", model: "gpt-6-astra"}} =
             Agents.set(name, harness: "codex", model: "gpt-6-astra")

    # only the model this time: the harness choice stays
    assert {:ok, %{harness: "codex", model: "o4"}} = Agents.set(name, model: "o4")
    assert %{harness: "codex", model: "o4"} = Agents.get(name)

    overrides =
      Application.fetch_env!(:aperture, :agent_overrides) |> File.read!() |> JSON.decode!()

    assert overrides[name] == %{"harness" => "codex", "model" => "o4"}

    # Claude Code's 1M-context aliases
    assert {:ok, %{model: "sonnet[1m]"}} =
             Agents.set(name, harness: "claude", model: "sonnet[1m]")
  end

  test "unknown agents, harnesses and unsafe models are refused" do
    name = AgentFixtures.define(AgentFixtures.unique("bad"))

    assert {:error, :unknown_agent} = Agents.set("nobody-here", harness: "codex")
    assert {:error, :invalid_harness} = Agents.set(name, harness: "cursor")
    assert {:error, :invalid_model} = Agents.set(name, model: "x; rm -rf /")
    assert %{harness: "claude", model: "opus"} = Agents.get(name)
  end

  test "an agent's own MCP servers: ~ expanded, unsafe names and the aperture name refused" do
    name =
      AgentFixtures.define(AgentFixtures.unique("m"), %{
        "mcp" => %{
          "mempalace" => %{
            "command" => "~/.mempalace/venv/bin/python",
            "args" => ["--palace", "~/.aperture/mempalace"]
          },
          "aperture" => %{"command" => "evil"},
          "bad name" => %{"command" => "x"},
          "no-command" => %{"args" => ["x"]}
        }
      })

    home = System.user_home!()

    assert Agents.get(name).mcp == %{
             "mempalace" => %{
               command: Path.join(home, ".mempalace/venv/bin/python"),
               args: ["--palace", Path.join(home, ".aperture/mempalace")],
               env: %{}
             }
           }
  end

  test "the repo ships GLaDOS on Claude Code and Peppy and Wheatley on Codex" do
    root = Application.fetch_env!(:aperture, :root)

    shipped =
      for name <- ~w(glados peppy wheatley), into: %{} do
        d = Path.join(root, "agents/#{name}.json") |> File.read!() |> JSON.decode!()
        {name, {d["harness"], d["model"], Map.keys(d["mcp"] || %{})}}
      end

    assert shipped == %{
             "glados" => {"claude", "opus", ["mempalace"]},
             "peppy" => {"codex", "gpt-6-astra", ["mempalace"]},
             "wheatley" => {"codex", "gpt-6-astra", ["mempalace"]}
           }
  end
end
