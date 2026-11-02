defmodule Aperture.DeliveryTest do
  @moduledoc """
  M1 behaviours from the v4 spec. Real hub, real sockets, fake agents.
  """
  use ExUnit.Case, async: false

  alias Aperture.FakeAgent

  # Unique agent names per test so tests never see each other's messages.
  # Fake agents are linked to the test process and die with it.
  setup context do
    suffix = "#{context.line}-#{System.unique_integer([:positive])}"
    {:ok, a: "a-#{suffix}", b: "b-#{suffix}"}
  end

  defp restart_child(id) do
    :ok = Supervisor.terminate_child(Aperture.Supervisor, id)
    {:ok, _} = Supervisor.restart_child(Aperture.Supervisor, id)
  end

  test "a message from A to B arrives exactly once", %{a: a, b: b} do
    fa = FakeAgent.start(a)
    fb = FakeAgent.start(b)

    FakeAgent.send_message(fa, b, "hello B")

    assert FakeAgent.wait_for(fb, 1) == ["hello B"]
    FakeAgent.wait_until(fn -> length(FakeAgent.sent(fa)) == 1 end, "sender got no sent ack")
    assert [%{"status" => "pushed"}] = FakeAgent.sent(fa)

    Process.sleep(200)
    assert FakeAgent.received(fb) == ["hello B"]
  end

  test "B offline when A sends: B gets it on connect", %{a: a, b: b} do
    fa = FakeAgent.start(a)

    FakeAgent.send_message(fa, b, "while you were out")
    FakeAgent.wait_until(fn -> length(FakeAgent.sent(fa)) == 1 end, "sender got no sent ack")
    assert [%{"status" => "queued"}] = FakeAgent.sent(fa)

    fb = FakeAgent.start(b)
    assert FakeAgent.wait_for(fb, 1) == ["while you were out"]
  end

  test "B's socket drops mid-burst: every message arrives, in order, once", %{a: a, b: b} do
    fa = FakeAgent.start(a)
    fb = FakeAgent.start(b)
    expected = for i <- 1..200, do: "m#{i}"

    for body <- expected, do: FakeAgent.send_message(fa, b, body)
    FakeAgent.wait_until(fn -> length(FakeAgent.received(fb)) >= 20 end, "burst never started")
    FakeAgent.drop(fb)

    assert FakeAgent.wait_for(fb, 200) == expected
    assert FakeAgent.welcomes(fb) >= 2, "B should have reconnected"
  end

  test "hub restart: clients reconnect and nothing queued is lost", %{a: a, b: b} do
    fa = FakeAgent.start(a)

    for i <- 1..5, do: FakeAgent.send_message(fa, b, "before #{i}")
    FakeAgent.wait_until(fn -> length(FakeAgent.sent(fa)) == 5 end, "sends not stored")

    # Bounce the endpoint (all sockets die) and the store (reopens the SQLite file).
    restart_child(:endpoint)
    restart_child(Aperture.Store)

    FakeAgent.wait_until(fn -> FakeAgent.welcomes(fa) >= 2 end, "A did not reconnect")

    fb = FakeAgent.start(b)
    FakeAgent.send_message(fa, b, "after")

    assert FakeAgent.wait_for(fb, 6) == [
             "before 1",
             "before 2",
             "before 3",
             "before 4",
             "before 5",
             "after"
           ]
  end

  test "acked messages are not redelivered after a reconnect", %{a: a, b: b} do
    fa = FakeAgent.start(a)
    fb = FakeAgent.start(b)

    for i <- 1..3, do: FakeAgent.send_message(fa, b, "m#{i}")
    assert FakeAgent.wait_for(fb, 3) == ["m1", "m2", "m3"]
    # let the acks land before dropping
    Process.sleep(100)

    FakeAgent.drop(fb)
    FakeAgent.wait_until(fn -> FakeAgent.welcomes(fb) >= 2 end, "B did not reconnect")
    Process.sleep(100)

    assert FakeAgent.frames(fb) == 3
  end
end
