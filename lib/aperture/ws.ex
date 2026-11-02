defmodule Aperture.WS do
  @moduledoc """
  Minimal WebSocket client on Mint, for use inside a GenServer.

  `connect/2` does the HTTP upgrade synchronously, then switches the socket to
  active mode: incoming data arrives as process messages that the owner hands
  to `handle_message/2`. Pings are answered automatically.

  Targets: `{:tcp, host, port}` or `{:unix, path}` (Codex app-server).
  """

  defstruct [:conn, :ref, :websocket]

  @type t :: %__MODULE__{}
  @type target :: {:tcp, String.t(), :inet.port_number()} | {:unix, Path.t()}
  @type frame :: {:text, String.t()} | {:close, integer() | nil, String.t()}

  @connect_timeout 5_000

  @spec connect(target(), String.t()) :: {:ok, t()} | {:error, term()}
  def connect({:tcp, host, port}, path), do: do_connect(host, port, path)
  def connect({:unix, sock}, path), do: do_connect({:local, sock}, 0, path)

  defp do_connect(address, port, path) do
    with {:ok, conn} <-
           Mint.HTTP.connect(:http, address, port,
             mode: :passive,
             protocols: [:http1],
             hostname: "localhost",
             transport_opts: [timeout: @connect_timeout]
           ),
         {:ok, conn, ref} <- Mint.WebSocket.upgrade(:ws, conn, path, []),
         {:ok, conn, status, headers} <- await_upgrade(conn, ref, nil, []),
         {:ok, conn, websocket} <- Mint.WebSocket.new(conn, ref, status, headers),
         {:ok, conn} <- Mint.HTTP.set_mode(conn, :active) do
      {:ok, %__MODULE__{conn: conn, ref: ref, websocket: websocket}}
    else
      {:error, reason} ->
        {:error, reason}

      {:error, conn, reason} ->
        close_quietly(conn)
        {:error, reason}

      {:error, conn, reason, _responses} ->
        close_quietly(conn)
        {:error, reason}
    end
  end

  defp await_upgrade(conn, ref, status, headers) do
    case Mint.HTTP.recv(conn, 0, @connect_timeout) do
      {:ok, conn, responses} ->
        {status, headers, done?} =
          Enum.reduce(responses, {status, headers, false}, fn
            {:status, ^ref, s}, {_, h, d} -> {s, h, d}
            {:headers, ^ref, h}, {s, acc, d} -> {s, acc ++ h, d}
            {:done, ^ref}, {s, h, _} -> {s, h, true}
            _, acc -> acc
          end)

        if done?,
          do: {:ok, conn, status, headers},
          else: await_upgrade(conn, ref, status, headers)

      {:error, conn, reason, _} ->
        {:error, conn, reason}
    end
  end

  @spec send_text(t(), String.t()) :: {:ok, t()} | {:error, t(), term()}
  def send_text(%__MODULE__{} = ws, text), do: send_frame(ws, {:text, text})

  @doc "Handle a process message. `:unknown` means it was not for this socket."
  @spec handle_message(t(), term()) ::
          {:ok, t(), [String.t()]} | {:closed, t(), term()} | :unknown
  def handle_message(%__MODULE__{} = ws, message) do
    case Mint.WebSocket.stream(ws.conn, message) do
      :unknown ->
        :unknown

      {:ok, conn, responses} ->
        ws = %{ws | conn: conn}

        data =
          for {:data, ref, bytes} <- responses, ref == ws.ref, into: <<>>, do: bytes

        decode(ws, data)

      {:error, conn, reason, _} ->
        {:closed, %{ws | conn: conn}, reason}
    end
  end

  defp decode(ws, <<>>), do: {:ok, ws, []}

  defp decode(ws, data) do
    case Mint.WebSocket.decode(ws.websocket, data) do
      {:ok, websocket, frames} ->
        ws = %{ws | websocket: websocket}
        handle_frames(ws, frames, [])

      {:error, websocket, reason} ->
        {:closed, %{ws | websocket: websocket}, reason}
    end
  end

  defp handle_frames(ws, [], texts), do: {:ok, ws, Enum.reverse(texts)}

  defp handle_frames(ws, [{:text, text} | rest], texts),
    do: handle_frames(ws, rest, [text | texts])

  defp handle_frames(ws, [{:ping, payload} | rest], texts) do
    case send_frame(ws, {:pong, payload}) do
      {:ok, ws} -> handle_frames(ws, rest, texts)
      {:error, ws, reason} -> {:closed, ws, reason}
    end
  end

  defp handle_frames(ws, [{:close, code, reason} | _], _texts) do
    _ = send_frame(ws, :close)
    {:closed, ws, {:remote_close, code, reason}}
  end

  defp handle_frames(ws, [_other | rest], texts), do: handle_frames(ws, rest, texts)

  @doc "Close from our side (best effort)."
  @spec close(t() | nil) :: :ok
  def close(nil), do: :ok

  def close(%__MODULE__{} = ws) do
    _ = send_frame(ws, :close)
    close_quietly(ws.conn)
    :ok
  end

  defp send_frame(ws, frame) do
    with {:ok, websocket, data} <- Mint.WebSocket.encode(ws.websocket, frame),
         ws = %{ws | websocket: websocket},
         {:ok, conn} <- Mint.WebSocket.stream_request_body(ws.conn, ws.ref, data) do
      {:ok, %{ws | conn: conn}}
    else
      {:error, %Mint.WebSocket{} = websocket, reason} ->
        {:error, %{ws | websocket: websocket}, reason}

      {:error, conn, reason} ->
        {:error, %{ws | conn: conn}, reason}
    end
  end

  defp close_quietly(conn) do
    _ = Mint.HTTP.close(conn)
    :ok
  end
end
