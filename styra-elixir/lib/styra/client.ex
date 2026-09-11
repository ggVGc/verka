defmodule Styra.Client do
  @moduledoc ~S"""
  The two things `Styra.Protocol` deliberately leaves to its callers.

  The generated module is the wire vocabulary and nothing else: it builds and
  checks request maps, and has no opinion about how those maps become bytes or
  how the bytes reach the server. That is the right shape for a generated
  library — but every script talking to Styra then needs the same two pieces, a
  JSON codec and a Unix socket, and writing them again per script is how a set
  of small tools drifts apart.

  So they live here instead, hand-written and not generated, sitting on top of
  `Styra.Protocol` exactly as a caller would:

      {:ok, styra} = Styra.Client.new()
      {:ok, health} = Styra.Client.call(styra, Request.health(), Response.health())

  Nothing is opened by `new/1`, which is why it is not called `connect`: a
  connection carries exactly one request and one response, so each exchange is
  a whole connection. What `new/1` returns is where to reach the server and how
  to encode for it.

  Neither piece is the protocol's, and both are replaceable — `:json` and
  `:exchange` take your own. The socket needs no library beyond OTP, which
  speaks Unix sockets natively through `:gen_tcp`.
  """

  alias Styra.Protocol

  @default_timeout 30_000

  defstruct socket: nil, json: nil, timeout: @default_timeout, exchange: nil

  @type t :: %__MODULE__{
          socket: String.t(),
          json: module(),
          timeout: timeout(),
          exchange: (String.t(), String.t(), timeout() -> {:ok, String.t()} | {:error, String.t()})
        }

  @doc ~S"""
  A client.

  Options:

    * `:socket` — the server's socket, defaulting to `default_socket/0`
    * `:json` — the codec module, needing `encode!/1` and `decode!/1`
    * `:timeout` — milliseconds for connecting and for reading, default 30_000
    * `:exchange` — a function replacing the transport entirely, which is what
      a test, or a script running somewhere with its own way to move bytes,
      wants

  """
  @spec new(keyword()) :: {:ok, t()} | {:error, String.t()}
  def new(options \\ []) do
    with {:ok, socket} <- socket_from(options),
         {:ok, json} <- codec_from(options) do
      {:ok,
       %__MODULE__{
         socket: socket,
         json: json,
         timeout: Keyword.get(options, :timeout, @default_timeout),
         exchange: Keyword.get(options, :exchange, &__MODULE__.exchange/3)
       }}
    end
  end

  @doc "`new/1`, raising `ArgumentError` instead of returning its message."
  @spec new!(keyword()) :: t()
  def new!(options \\ []) do
    case new(options) do
      {:ok, client} -> client
      {:error, message} -> raise ArgumentError, message
    end
  end

  @doc ~S"""
  The socket `styractl` uses, unless told otherwise.
  """
  @spec default_socket() :: {:ok, String.t()} | {:error, String.t()}
  def default_socket do
    case System.get_env("XDG_RUNTIME_DIR") do
      nil -> {:error, "XDG_RUNTIME_DIR is not set; pass :socket explicitly"}
      "" -> {:error, "XDG_RUNTIME_DIR is empty; pass :socket explicitly"}
      runtime -> {:ok, Path.join([runtime, "styra", "styra.sock"])}
    end
  end

  @doc ~S"""
  Send one request and return the decoded reply, whatever it says.

  Named `request` rather than `send` because `Kernel.send/2` is a message to a
  process and shadowing it would be a poor trade for one word.

  What is passed may be a map or the `{:ok, request}` a constructor returns, so
  the two read the same at a call site:

      Styra.Client.request(styra, Styra.Protocol.Request.health())
  """
  @spec request(t(), map() | {:ok, map()} | {:error, String.t()}) ::
          {:ok, term()} | {:error, String.t()}
  def request(client, request)
  def request(_client, {:error, message}), do: {:error, message}
  def request(client, {:ok, request}), do: request(client, request)

  def request(%__MODULE__{} = client, request) when is_map(request) do
    with {:ok, reply} <- client.exchange.(client.socket, client.json.encode!(request), client.timeout) do
      {:ok, client.json.decode!(reply)}
    end
  end

  @doc ~S"""
  Send one request and return the data of the response it must answer with.

  The expected response is named by the caller so a surprise comes back as a
  message rather than a missing key three lines later.

      {:ok, workspaces} =
        Styra.Client.call(styra, Request.list_workspaces(), Response.workspaces())
  """
  @spec call(t(), map() | {:ok, map()} | {:error, String.t()}, String.t()) ::
          {:ok, term()} | {:error, String.t()}
  def call(client, wire, expected) do
    with {:ok, reply} <- request(client, wire) do
      Protocol.expect(reply, expected)
    end
  end

  @doc ~S"""
  Send one request and return the whole response, for the callers that
  genuinely handle more than one outcome.
  """
  @spec unwrap(t(), map() | {:ok, map()} | {:error, String.t()}) ::
          {:ok, term()} | {:error, String.t()}
  def unwrap(client, wire) do
    with {:ok, reply} <- request(client, wire) do
      Protocol.unwrap(reply)
    end
  end

  @doc ~S"""
  Write one line to the socket and read one back.

  A connection carries exactly one request and one response, so an exchange is
  a whole connection: connect, write one line, read one line, done. OTP speaks
  Unix sockets itself, so this needs nothing installed.
  """
  @spec exchange(String.t(), iodata(), timeout()) :: {:ok, String.t()} | {:error, String.t()}
  def exchange(path, line, timeout \\ @default_timeout) do
    address = {:local, String.to_charlist(path)}
    options = [:binary, active: false, packet: :line]

    case :gen_tcp.connect(address, 0, options, timeout) do
      {:ok, socket} ->
        try do
          exchange_on(socket, line, timeout, path)
        after
          :gen_tcp.close(socket)
        end

      {:error, reason} ->
        {:error, "connecting to #{path}: #{describe(reason)}"}
    end
  end

  defp exchange_on(socket, line, timeout, path) do
    with :ok <- :gen_tcp.send(socket, [line, "\n"]),
         {:ok, reply} <- :gen_tcp.recv(socket, 0, timeout) do
      {:ok, reply}
    else
      {:error, :closed} -> {:error, "#{path} closed the connection without replying"}
      {:error, reason} -> {:error, "talking to #{path}: #{describe(reason)}"}
    end
  end

  defp socket_from(options) do
    case Keyword.get(options, :socket) do
      nil -> default_socket()
      socket -> {:ok, socket}
    end
  end

  # Elixir has carried a JSON module since 1.18; Jason is what a project on an
  # older one almost certainly already has.
  defp codec_from(options) do
    case Keyword.get(options, :json) do
      nil ->
        cond do
          Code.ensure_loaded?(JSON) -> {:ok, JSON}
          Code.ensure_loaded?(Jason) -> {:ok, Jason}
          true -> {:error, "no JSON codec; use Elixir 1.18 or later, or add :jason"}
        end

      json ->
        {:ok, json}
    end
  end

  defp describe(reason) when is_atom(reason) do
    case :inet.format_error(reason) do
      ~c"unknown POSIX error" -> inspect(reason)
      described -> List.to_string(described)
    end
  end

  defp describe(reason), do: inspect(reason)
end
