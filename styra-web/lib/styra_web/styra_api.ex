defmodule StyraWeb.StyraAPI do
  @moduledoc """
  The small application-facing layer over `Styra.Client` and the generated
  `Styra.Protocol` vocabulary.

  Keeping socket calls here leaves the LiveView responsible only for UI state.
  """

  alias Styra.Client
  alias Styra.Protocol.{Request, Response}

  @timeout 1_500

  @spec default_socket() :: String.t()
  def default_socket do
    case Client.default_socket() do
      {:ok, path} -> path
      {:error, _message} -> "/tmp/styra/styra.sock"
    end
  end

  @spec snapshot(String.t(), String.t() | nil, non_neg_integer()) ::
          {:ok, map()} | {:error, String.t()}
  def snapshot(socket_path, selected_id, cursor) do
    with {:ok, client} <- client(socket_path),
         {:ok, health} <- Client.call(client, Request.health(), Response.health()),
         {:ok, interactions} <-
           Client.call(client, Request.list_interactions(), Response.interactions()),
         {:ok, updates} <- updates(client, selected_id, cursor) do
      {:ok,
       %{
         service: health["service"],
         interactions: interactions,
         selected_id: selected_id,
         cursor: updates["next"],
         updates: updates["updates"]
       }}
    end
  rescue
    error -> {:error, Exception.message(error)}
  end

  @spec send_message(String.t(), String.t(), String.t(), String.t()) ::
          :ok | {:error, String.t()}
  def send_message(socket_path, id, text, contract) do
    message =
      %{text: text}
      |> maybe_put_contract(contract)

    with {:ok, client} <- client(socket_path),
         {:ok, _accepted} <-
           Client.call(
             client,
             Request.send_message(%{id: id, message: message}),
             Response.accepted()
           ) do
      :ok
    end
  rescue
    error -> {:error, Exception.message(error)}
  end

  @spec action(String.t(), String.t(), String.t()) :: :ok | {:error, String.t()}
  def action(socket_path, id, "interrupt") do
    accepted(socket_path, Request.interrupt_interaction(%{id: id}))
  end

  def action(socket_path, id, "stop") do
    accepted(socket_path, Request.stop_interaction(%{id: id}))
  end

  def action(_socket_path, _id, action), do: {:error, "unknown action: #{action}"}

  defp accepted(socket_path, request) do
    with {:ok, client} <- client(socket_path),
         {:ok, _accepted} <- Client.call(client, request, Response.accepted()) do
      :ok
    end
  end

  defp client(socket_path) do
    options = Application.get_env(:styra_web, :styra_client_options, [])
    Client.new(Keyword.merge(options, socket: socket_path, timeout: @timeout))
  end

  defp updates(_client, nil, cursor), do: {:ok, %{"next" => cursor, "updates" => []}}

  defp updates(client, id, cursor) do
    Client.call(
      client,
      Request.updates(%{id: id, after: cursor, raw: false}),
      Response.updates()
    )
  end

  defp maybe_put_contract(message, "none"), do: message
  defp maybe_put_contract(message, contract), do: Map.put(message, :contract, contract)
end
