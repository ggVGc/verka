# A small Styra client, written against the generated protocol library.
#
#   mix run examples/styra_ask.exs health
#   mix run examples/styra_ask.exs workspaces
#   mix run examples/styra_ask.exs interactions
#   mix run examples/styra_ask.exs ask styra-1 which files handle auth?
#
# It exists to show what the library does and does not do. Every operation
# name, field name, and enum spelling below comes from `Styra.Protocol`, which
# is generated from the Rust that defines the protocol — nothing here restates
# the wire format, and a request this file gets wrong is refused by the line
# that built it rather than by the server a round trip later.
#
# The JSON codec and the socket are the two things the protocol leaves to its
# callers, and `Styra.Client` is where this repository keeps them. Point this
# at a server with --socket PATH; the default is
# $XDG_RUNTIME_DIR/styra/styra.sock, the same one `styractl` uses.

alias Styra.Client
alias Styra.Protocol.{AgentEvent, Contract, InteractionActivity, InteractionUpdate, Request}
alias Styra.Protocol.Response

defmodule StyraAsk do
  @moduledoc false

  def health(styra) do
    with {:ok, health} <- Client.call(styra, Request.health(), Response.health()) do
      IO.puts("#{health["service"]} is up")
    end
  end

  def workspaces(styra) do
    with {:ok, workspaces} <-
           Client.call(styra, Request.list_workspaces(), Response.workspaces()) do
      for workspace <- workspaces do
        IO.puts(
          "#{workspace["id"]}  #{pad(workspace["name"] || "-", 30)} #{workspace["host_path"]}" <>
            "  (#{workspace["session_count"]} sessions)"
        )
      end

      :ok
    end
  end

  def interactions(styra) do
    with {:ok, interactions} <-
           Client.call(styra, Request.list_interactions(), Response.interactions()) do
      for interaction <- interactions do
        selection = interaction["selection"]

        IO.puts(
          "#{interaction["id"]}  " <>
            "#{pad(interaction["activity"] || InteractionActivity.pending(), 10)} " <>
            "#{selection["provider"]}:#{selection["model"]}/#{selection["effort"]}  " <>
            "#{interaction["last_message"] || ""}"
        )
      end

      :ok
    end
  end

  @doc """
  Ask one question under a contract, wait for the turn, print the answer.

  The three requests this takes are the protocol's own shape, not a quirk of
  Elixir: a connection carries one request, and a turn takes minutes, so the
  answer is fetched rather than returned. `send_message` names the contract,
  `updates` is polled until the turn ends, and `turn_answer` parses the reply
  the server already has.
  """
  def ask(styra, [session | words]) when words != [] do
    question = Enum.join(words, " ")

    with {:ok, _accepted} <-
           Client.call(
             styra,
             Request.send_message(%{
               id: session,
               message: %{text: question, contract: Contract.lines()}
             }),
             Response.accepted()
           ),
         :ok <- await_turn(styra, session, 0),
         {:ok, answer} <-
           Client.call(styra, Request.turn_answer(%{id: session}), Response.answer()) do
      print_answer(answer)
    end
  end

  def ask(_styra, _arguments), do: {:error, "usage: ask <session> <question...>"}

  defp await_turn(styra, session, after_sequence) do
    # The raw wire lines dominate a long interaction's volume and this client
    # renders none of them.
    with {:ok, updates} <-
           Client.call(
             styra,
             Request.updates(%{id: session, after: after_sequence, raw: false}),
             Response.updates()
           ) do
      case Enum.reduce_while(updates["updates"], :waiting, &examine/2) do
        :waiting ->
          Process.sleep(250)
          await_turn(styra, session, updates["next"])

        :ended ->
          :ok

        {:error, _message} = error ->
          error
      end
    end
  end

  # The spellings are pinned rather than called in the patterns: a remote call
  # is not allowed in a guard, and matching on `Contract.lines()` directly
  # would bind a variable named for the thing it was meant to compare with.
  defp examine(sequenced, _state) do
    update = sequenced["update"]
    completed = AgentEvent.turn_completed()
    failed = AgentEvent.error()

    cond do
      update["type"] == InteractionUpdate.ended() ->
        {:halt, {:error, "the interaction ended before it answered"}}

      update["type"] != InteractionUpdate.event() ->
        {:cont, :waiting}

      true ->
        event = update["data"]

        case event["type"] do
          ^completed -> {:halt, :ended}
          ^failed -> {:halt, {:error, event["message"]}}
          _ -> {:cont, :waiting}
        end
    end
  end

  # A reply that missed its contract is still an answer: show what was said and
  # why it could not be read, rather than nothing at all.
  defp print_answer(%{"value" => nil} = answer) do
    IO.write(:stderr, "the reply did not satisfy its contract: #{answer["error"]}\n")
    IO.puts(answer["source"])
  end

  defp print_answer(answer) do
    for item <- answer["value"]["value"], do: IO.puts(item)
    :ok
  end

  defp pad(text, width), do: String.pad_trailing(to_string(text), width)
end

# Entry point ---------------------------------------------------------------

{socket, arguments} =
  case System.argv() do
    ["--socket", path | rest] -> {path, rest}
    arguments -> {nil, arguments}
  end

usage = """
usage: styra_ask.exs [--socket PATH] <command> [arguments]

  health                        check that the server is reachable
  workspaces                    list Workspaces
  interactions                  list live interactions
  ask <session> <question...>   ask one question and print the typed answer
"""

options = if socket, do: [socket: socket], else: []

result =
  with {:ok, styra} <- Client.new(options) do
    case arguments do
      ["health"] -> StyraAsk.health(styra)
      ["workspaces"] -> StyraAsk.workspaces(styra)
      ["interactions"] -> StyraAsk.interactions(styra)
      ["ask" | rest] -> StyraAsk.ask(styra, rest)
      _ -> {:usage, usage}
    end
  end

case result do
  {:usage, usage} ->
    IO.write(:stderr, usage)
    System.halt(2)

  {:error, message} ->
    IO.write(:stderr, "#{message}\n")
    System.halt(1)

  _ ->
    :ok
end
