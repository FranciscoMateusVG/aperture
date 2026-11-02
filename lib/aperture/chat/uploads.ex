defmodule Aperture.Chat.Uploads do
  @moduledoc """
  Images the operator attaches to chat messages.

  Only PNG, JPEG, WebP and GIF are accepted, judged by their magic bytes (never
  by the client's file name or content type), up to `max_bytes/0`. Each file
  is stored in the uploads dir (config `:uploads_dir`) under a generated id,
  `<32 hex>.<ext>`, so nothing the client sent ends up in a path. `path/1`
  only resolves ids of that exact shape, which rules out traversal.
  """

  @max_bytes 10 * 1024 * 1024
  @id_re ~r/\A[0-9a-f]{32}\.(png|jpg|webp|gif)\z/

  @type t :: %{id: String.t(), url: String.t(), path: Path.t(), mime: String.t(), size: integer()}
  @type error :: :too_large | :bad_type

  @spec max_bytes() :: pos_integer()
  def max_bytes, do: @max_bytes

  @spec dir() :: Path.t()
  def dir, do: Application.fetch_env!(:aperture, :uploads_dir)

  @doc "Validate and store the file at `tmp_path` (e.g. a LiveView upload's temp file)."
  @spec store_file(Path.t()) :: {:ok, t()} | {:error, error()}
  def store_file(tmp_path) do
    case File.stat(tmp_path) do
      {:ok, %{type: :regular, size: size}} when size > @max_bytes -> {:error, :too_large}
      {:ok, %{type: :regular}} -> tmp_path |> File.read!() |> store_binary()
      _ -> {:error, :bad_type}
    end
  end

  @doc "Validate and store image bytes."
  @spec store_binary(binary()) :: {:ok, t()} | {:error, error()}
  def store_binary(data) when byte_size(data) > @max_bytes, do: {:error, :too_large}

  def store_binary(data) when is_binary(data) do
    case sniff(data) do
      nil ->
        {:error, :bad_type}

      {ext, mime} ->
        id = Base.encode16(:crypto.strong_rand_bytes(16), case: :lower) <> "." <> ext
        File.mkdir_p!(dir())
        path = Path.join(dir(), id)
        File.write!(path, data, [:exclusive, :binary])
        {:ok, info(id, path, mime, byte_size(data))}
    end
  end

  @doc "The stored upload `id`, or `:error` for anything that is not one."
  @spec fetch(term()) :: {:ok, t()} | :error
  def fetch(id) do
    with {:ok, path} <- path(id),
         {:ok, %{type: :regular, size: size}} <- File.stat(path) do
      {:ok, info(id, path, mime(id), size)}
    else
      _ -> :error
    end
  end

  @doc "Absolute path for a well-formed upload id (the file may not exist)."
  @spec path(term()) :: {:ok, Path.t()} | :error
  def path(id) when is_binary(id) do
    if Regex.match?(@id_re, id), do: {:ok, Path.join(dir(), id)}, else: :error
  end

  def path(_), do: :error

  @spec mime(String.t()) :: String.t()
  def mime(id) do
    case Path.extname(id) do
      ".png" -> "image/png"
      ".jpg" -> "image/jpeg"
      ".webp" -> "image/webp"
      ".gif" -> "image/gif"
    end
  end

  defp info(id, path, mime, size),
    do: %{id: id, url: "/uploads/" <> id, path: path, mime: mime, size: size}

  defp sniff(<<0x89, "PNG", 0x0D, 0x0A, 0x1A, 0x0A, _::binary>>), do: {"png", "image/png"}
  defp sniff(<<0xFF, 0xD8, 0xFF, _::binary>>), do: {"jpg", "image/jpeg"}
  defp sniff(<<"GIF8", v, "a", _::binary>>) when v in [?7, ?9], do: {"gif", "image/gif"}
  defp sniff(<<"RIFF", _size::binary-size(4), "WEBP", _::binary>>), do: {"webp", "image/webp"}
  defp sniff(_), do: nil
end
