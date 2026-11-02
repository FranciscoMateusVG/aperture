defmodule Aperture.ClientsTest do
  @moduledoc """
  The real Node clients (`bin/aperture-inbox.mjs`, `bin/aperture-mcp.mjs`)
  against the real hub. Skipped when `node` (22+) is not installed.
  """
  use ExUnit.Case, async: false

  alias Aperture.{FakeAgent, NodeClient}

  @moduletag skip: if(NodeClient.node_path(), do: false, else: "node not installed")

  setup context do
    suffix = "#{context.line}-#{System.unique_integer([:positive])}"
    port = Application.fetch_env!(:aperture, :port)

    {:ok,
     a: "a-#{suffix}",
     b: "b-#{suffix}",
     ws: "ws://127.0.0.1:#{port}/inbox",
     http: "http://127.0.0.1:#{port}"}
  end

  defp inbox(agent, ws) do
    client = NodeClient.start("aperture-inbox.mjs", ["--agent", agent, "--url", ws])
    on_exit(fn -> NodeClient.kill(client) end)
    # registration is visible through the public /agents view
    FakeAgent.wait_until(
      fn -> agent in Aperture.Router.connected_agents() end,
      "#{agent} inbox never connected"
    )

    client
  end

  defp restart_endpoint do
    :ok = Supervisor.terminate_child(Aperture.Supervisor, :endpoint)
    {:ok, _} = Supervisor.restart_child(Aperture.Supervisor, :endpoint)
  end

  test "inbox prints each message once, as one line", %{a: a, b: b, ws: ws} do
    fa = FakeAgent.start(a)
    inbox_b = inbox(b, ws)

    FakeAgent.send_message(fa, b, "line one\nline two")

    line = NodeClient.wait_line(inbox_b, &String.contains?(&1, "from #{a}"))
    assert line =~ ~r/^\[aperture\] message #\d+ from #{a}: line one\\nline two$/
    Process.sleep(200)
    assert Enum.count(NodeClient.lines(inbox_b), &String.contains?(&1, "from #{a}")) == 1
  end

  test "inbox survives a hub restart and keeps receiving", %{a: a, b: b, ws: ws} do
    fa = FakeAgent.start(a)
    inbox_b = inbox(b, ws)

    restart_endpoint()

    FakeAgent.wait_until(
      fn -> b in Aperture.Router.connected_agents() end,
      "inbox did not reconnect",
      8_000
    )

    FakeAgent.wait_until(fn -> FakeAgent.welcomes(fa) >= 2 end, "A did not reconnect", 10_000)

    # the inbox may still be backing off (it can see the old registration
    # above before it drops); the message waits in the store until it's back
    FakeAgent.send_message(fa, b, "after restart")
    assert NodeClient.wait_line(inbox_b, &String.ends_with?(&1, ": after restart"), 10_000)
  end

  test "inbox acks: a restarted inbox does not reprint old messages", %{a: a, b: b, ws: ws} do
    fa = FakeAgent.start(a)
    first = inbox(b, ws)

    FakeAgent.send_message(fa, b, "old news")
    NodeClient.wait_line(first, &String.ends_with?(&1, ": old news"))
    Process.sleep(100)
    NodeClient.kill(first)

    second = inbox(b, ws)
    FakeAgent.send_message(fa, b, "fresh")
    NodeClient.wait_line(second, &String.ends_with?(&1, ": fresh"))
    refute Enum.any?(NodeClient.lines(second), &String.ends_with?(&1, ": old news"))
  end

  test "MCP send_message delivers to another agent", %{a: a, b: b, http: http} do
    fb = FakeAgent.start(b)
    mcp = NodeClient.start("aperture-mcp.mjs", ["--agent", a, "--hub", http])
    on_exit(fn -> NodeClient.kill(mcp) end)

    rpc = fn id, method, params ->
      NodeClient.write(
        mcp,
        JSON.encode!(%{jsonrpc: "2.0", id: id, method: method, params: params}) <> "\n"
      )

      line = NodeClient.wait_line(mcp, &String.contains?(&1, ~s("id":#{id})))
      JSON.decode!(line)
    end

    assert %{"result" => %{"serverInfo" => %{"name" => "aperture"}}} =
             rpc.(1, "initialize", %{
               protocolVersion: "2025-06-18",
               capabilities: %{},
               clientInfo: %{name: "test", version: "0"}
             })

    NodeClient.write(mcp, ~s({"jsonrpc":"2.0","method":"notifications/initialized"}\n))

    assert %{"result" => %{"tools" => tools}} = rpc.(2, "tools/list", %{})

    assert Enum.map(tools, & &1["name"]) ==
             ~w(send_message list_agents create_task list_tasks show_task claim_task update_task close_task recall recall_full)

    assert %{"result" => %{"content" => [%{"text" => text}]}} =
             rpc.(3, "tools/call", %{name: "send_message", arguments: %{to: b, body: "via mcp"}})

    assert text =~ "pushed"
    assert FakeAgent.wait_for(fb, 1) == ["via mcp"]

    assert %{"result" => %{"content" => [%{"text" => agents}]}} =
             rpc.(4, "tools/call", %{name: "list_agents", arguments: %{}})

    assert agents =~ b
  end

  test "a newer inbox for the same agent replaces the old one, which exits", %{a: a, b: b, ws: ws} do
    fa = FakeAgent.start(a)
    old = inbox(b, ws)

    new = NodeClient.start("aperture-inbox.mjs", ["--agent", b, "--url", ws])
    on_exit(fn -> NodeClient.kill(new) end)

    FakeAgent.wait_until(fn -> not alive?(old) end, "old inbox did not exit")

    FakeAgent.send_message(fa, b, "to the new one")
    NodeClient.wait_line(new, &String.ends_with?(&1, ": to the new one"))

    # no flapping: the new inbox stays connected and alive
    Process.sleep(1_500)
    assert alive?(new)
    assert b in Aperture.Router.connected_agents()
  end

  defp alive?(%{os_pid: pid}) do
    {_, code} = System.cmd("kill", ["-0", to_string(pid)], stderr_to_stdout: true)
    code == 0
  end
end
