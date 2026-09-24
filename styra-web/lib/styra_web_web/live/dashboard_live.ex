defmodule StyraWebWeb.DashboardLive do
  use StyraWebWeb, :live_view

  alias Styra.Protocol.Contract
  alias StyraWeb.StyraAPI

  @poll_interval 1_000
  @max_updates 300
  @max_audio_size 5_000_000

  @impl true
  def mount(_params, _session, socket) do
    {socket_path, socket_error} =
      case StyraAPI.socket_path() do
        {:ok, path} -> {path, nil}
        {:error, message} -> {nil, message}
      end

    socket =
      socket
      |> stream_configure(:interactions, dom_id: &"interaction-#{&1["id"]}")
      |> stream_configure(:updates, dom_id: &"update-#{&1["sequence"]}")
      |> stream(:interactions, [])
      |> stream(:updates, [])
      |> assign(
        page_title: "Live interactions",
        socket_path: socket_path,
        message_form: message_form(),
        connected: false,
        service: nil,
        error: socket_error,
        interaction_count: 0,
        selected_id: nil,
        selected_interaction: nil,
        updates_empty?: true,
        cursor: 0,
        refreshing: false,
        audio_state: :idle,
        audio_contract: "none",
        audio_before: "",
        audio_after: ""
      )
      |> allow_upload(:audio,
        accept: ~w(.wav),
        max_entries: 1,
        max_file_size: @max_audio_size,
        auto_upload: true,
        progress: &handle_audio_progress/3
      )

    if connected?(socket) and socket_path, do: send(self(), :poll)
    {:ok, socket}
  end

  @impl true
  def handle_info(:poll, %{assigns: %{socket_path: nil}} = socket), do: {:noreply, socket}

  def handle_info(:poll, %{assigns: %{refreshing: true}} = socket), do: {:noreply, socket}

  def handle_info(:poll, socket) do
    path = socket.assigns.socket_path
    selected_id = socket.assigns.selected_id
    cursor = socket.assigns.cursor

    socket =
      socket
      |> assign(:refreshing, true)
      |> start_async(:refresh, fn ->
        {path, selected_id, cursor, StyraAPI.snapshot(path, selected_id, cursor)}
      end)

    {:noreply, socket}
  end

  @impl true
  def handle_async(:refresh, {:ok, {path, selected_id, cursor, result}}, socket) do
    current_request? =
      path == socket.assigns.socket_path and selected_id == socket.assigns.selected_id and
        cursor == socket.assigns.cursor

    socket =
      if current_request? do
        apply_snapshot(socket, result)
      else
        socket
      end

    schedule_poll(if(current_request?, do: @poll_interval, else: 0))
    {:noreply, assign(socket, :refreshing, false)}
  end

  def handle_async(:refresh, {:exit, reason}, socket) do
    schedule_poll(@poll_interval)

    {:noreply,
     socket
     |> assign(:refreshing, false)
     |> disconnected("refresh failed: #{inspect(reason)}")}
  end

  def handle_async({:voice_message, _ref}, {:ok, {:ok, transcript}}, socket) do
    text = socket.assigns.audio_before <> transcript <> socket.assigns.audio_after

    {:noreply,
     socket
     |> assign(
       audio_state: :idle,
       error: nil,
       message_form: message_form(socket.assigns.audio_contract, text)
     )
     |> push_event("voice-finished", %{})}
  end

  def handle_async({:voice_message, _ref}, {:ok, {:error, message}}, socket) do
    {:noreply, voice_error(socket, message)}
  end

  def handle_async({:voice_message, _ref}, {:exit, reason}, socket) do
    {:noreply, voice_error(socket, "Voice message failed: #{inspect(reason)}")}
  end

  @impl true
  def handle_event("select", %{"id" => id}, socket) do
    send(self(), :poll)

    {:noreply,
     socket
     |> assign(
       selected_id: id,
       selected_interaction: nil,
       updates_empty?: true,
       cursor: 0,
       error: nil,
       message_form: message_form()
     )
     |> stream(:updates, [], reset: true)}
  end

  def handle_event("deselect", _params, socket) do
    {:noreply,
     socket
     |> assign(
       selected_id: nil,
       selected_interaction: nil,
       updates_empty?: true,
       cursor: 0,
       error: nil
     )
     |> stream(:updates, [], reset: true)}
  end

  def handle_event("send", %{"message" => params}, socket) do
    text = String.trim(params["text"] || "")
    contract = params["contract"] || "none"

    cond do
      is_nil(socket.assigns.selected_id) ->
        {:noreply, assign(socket, :error, "Select an interaction first.")}

      text == "" ->
        {:noreply, assign(socket, :error, "Write a message first.")}

      true ->
        result =
          StyraAPI.send_message(
            socket.assigns.socket_path,
            socket.assigns.selected_id,
            text,
            contract
          )

        case result do
          :ok ->
            send(self(), :poll)
            {:noreply, assign(socket, message_form: message_form(contract), error: nil)}

          {:error, message} ->
            {:noreply, assign(socket, :error, message)}
        end
    end
  end

  def handle_event("audio_recording_started", _params, socket) do
    if socket.assigns.selected_id do
      case StyraAPI.audio_recording_started(socket.assigns.socket_path) do
        :ok ->
          {:noreply,
           assign(socket,
             audio_state: :recording,
             error: nil
           )}

        {:error, message} ->
          {:noreply, voice_error(socket, message)}
      end
    else
      {:noreply, voice_error(socket, "Select an interaction first.")}
    end
  end

  def handle_event("audio_recording_stopped", params, socket) do
    contract = normalize_contract(params["contract"])
    before = draft_part(params["before"])
    after_text = draft_part(params["after"])

    case StyraAPI.audio_recording_stopped(socket.assigns.socket_path) do
      :ok ->
        {:noreply,
         assign(socket,
           audio_state: :uploading,
           audio_contract: contract,
           audio_before: before,
           audio_after: after_text
         )}

      {:error, message} ->
        {:noreply, voice_error(socket, message)}
    end
  end

  def handle_event("audio_recording_error", %{"error" => error}, socket) do
    _ = StyraAPI.audio_transcription_error(socket.assigns.socket_path, error)
    {:noreply, voice_error(socket, error)}
  end

  def handle_event("validate_audio", _params, socket), do: {:noreply, socket}

  def handle_event("action", %{"name" => action}, socket) do
    case StyraAPI.action(socket.assigns.socket_path, socket.assigns.selected_id, action) do
      :ok ->
        send(self(), :poll)
        {:noreply, assign(socket, :error, nil)}

      {:error, message} ->
        {:noreply, assign(socket, :error, message)}
    end
  end

  defp apply_snapshot(socket, {:ok, snapshot}) do
    selected_still_exists? =
      is_nil(snapshot.selected_id) or
        Enum.any?(snapshot.interactions, &(&1["id"] == snapshot.selected_id))

    if selected_still_exists? do
      selected_interaction =
        Enum.find(snapshot.interactions, &(&1["id"] == snapshot.selected_id))

      socket
      |> assign(
        connected: true,
        service: snapshot.service,
        error: nil,
        interaction_count: length(snapshot.interactions),
        selected_interaction: selected_interaction,
        cursor: snapshot.cursor,
        updates_empty?: socket.assigns.updates_empty? and snapshot.updates == []
      )
      |> stream(:interactions, snapshot.interactions, reset: true)
      |> stream(:updates, snapshot.updates, at: -1, limit: -@max_updates)
    else
      socket
      |> assign(
        connected: true,
        service: snapshot.service,
        error: nil,
        interaction_count: length(snapshot.interactions),
        selected_id: nil,
        selected_interaction: nil,
        cursor: 0,
        updates_empty?: true
      )
      |> stream(:interactions, snapshot.interactions, reset: true)
      |> stream(:updates, [], reset: true)
    end
  end

  defp apply_snapshot(socket, {:error, message}), do: disconnected(socket, message)

  defp disconnected(socket, message) do
    assign(socket, connected: false, service: nil, error: message)
  end

  defp schedule_poll(delay), do: Process.send_after(self(), :poll, delay)

  defp handle_audio_progress(:audio, entry, socket) do
    if entry.done? do
      path =
        consume_uploaded_entry(socket, entry, fn %{path: upload_path} ->
          path =
            Path.join(
              System.tmp_dir!(),
              "styra-web-voice-#{System.unique_integer([:positive, :monotonic])}.wav"
            )

          case File.cp(upload_path, path) do
            :ok -> {:ok, path}
            {:error, reason} -> {:ok, {:copy_error, reason}}
          end
        end)

      case path do
        {:copy_error, reason} ->
          {:noreply,
           voice_error(socket, "Could not stage the recording: #{:file.format_error(reason)}")}

        path ->
          socket_path = socket.assigns.socket_path

          {:noreply,
           socket
           |> assign(audio_state: :transcribing, error: nil)
           |> start_async({:voice_message, entry.ref}, fn ->
             try do
               with {:ok, transcript} <- StyraAPI.transcribe_audio(socket_path, path),
                    transcript = String.trim(transcript),
                    :ok <- nonempty_transcript(transcript) do
                 {:ok, transcript}
               end
             after
               File.rm(path)
             end
           end)}
      end
    else
      {:noreply, assign(socket, :audio_state, :uploading)}
    end
  end

  defp nonempty_transcript(""), do: {:error, "Styra could not recognize any speech."}
  defp nonempty_transcript(_transcript), do: :ok

  defp normalize_contract(contract) when is_binary(contract) do
    if contract in ["none" | Contract.values()], do: contract, else: "none"
  end

  defp normalize_contract(_contract), do: "none"

  defp draft_part(value) when is_binary(value), do: value
  defp draft_part(_value), do: ""

  defp voice_error(socket, message) do
    socket
    |> assign(audio_state: :idle, error: message)
    |> push_event("voice-failed", %{message: message})
  end

  defp message_form(contract \\ "none", text \\ "") do
    to_form(%{"text" => text, "contract" => contract}, as: :message)
  end

  defp display_name(interaction) do
    interaction["name"] || interaction["id"]
  end

  defp selection_name(interaction) do
    selection = interaction["selection"] || %{}
    "#{selection["provider"] || "?"}:#{selection["model"] || "?"}/#{selection["effort"] || "?"}"
  end

  defp activity_class("running"), do: "bg-amber-500"
  defp activity_class("background"), do: "bg-sky-600"
  defp activity_class("pending"), do: "bg-emerald-600"
  defp activity_class(_activity), do: "bg-gray-400"

  defp update_kind(%{"update" => %{"type" => "event", "data" => event}}),
    do: event["type"] || "event"

  defp update_kind(%{"update" => %{"type" => type}}), do: type
  defp update_kind(_update), do: "update"

  defp update_text(%{"update" => %{"type" => "event", "data" => event}}) do
    event["text"] || event["message"] || event["detail"] || event["command"] ||
      event["output"] || event_summary(event)
  end

  defp update_text(%{"update" => %{"type" => "log", "data" => data}}),
    do: data["message"]

  defp update_text(%{"update" => %{"type" => "working_directory_changed", "data" => path}}),
    do: path

  defp update_text(%{"update" => %{"type" => "ended", "data" => data}}),
    do: data["error"] || "Process ended (exit #{inspect(data["exit_code"])})"

  defp update_text(%{"update" => update}), do: Jason.encode!(update, pretty: true)
  defp update_text(update), do: inspect(update)

  defp event_summary(event) do
    event
    |> Map.delete("type")
    |> case do
      map when map == %{} -> event["type"] |> String.replace("_", " ")
      map -> Jason.encode!(map, pretty: true)
    end
  end

  defp conversational?(update), do: update_kind(update) in ["user_message", "agent_message"]

  defp user_message?(update), do: update_kind(update) == "user_message"

  defp contract_options do
    [{"Unstructured", "none"}] ++ Enum.map(Contract.values(), &{String.capitalize(&1), &1})
  end

  defp audio_status(:recording), do: "Recording — tap the microphone to send"
  defp audio_status(:uploading), do: "Uploading recording…"
  defp audio_status(:transcribing), do: "Transcribing recording…"

  defp upload_error(:too_large), do: "The recording is too long. Try a shorter message."
  defp upload_error(:not_accepted), do: "The browser produced an unsupported audio format."
  defp upload_error(:too_many_files), do: "Only one voice message can be sent at a time."
  defp upload_error(error), do: "Could not upload the recording: #{inspect(error)}"
end
