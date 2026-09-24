defmodule StyraWebWeb.DashboardLiveTest do
  use StyraWebWeb.ConnCase, async: false

  import Phoenix.LiveViewTest

  setup do
    test_process = self()
    previous_socket_path = Application.get_env(:styra_web, :styra_socket_path)

    exchange = fn path, json, _timeout ->
      request = Jason.decode!(json)
      send(test_process, {:styra_socket, path})
      send(test_process, {:styra_request, request})

      response =
        case request["operation"] do
          "health" ->
            ok("health", %{"service" => "styra"})

          "list_interactions" ->
            ok("interactions", [interaction()])

          "updates" ->
            ok("updates", %{
              "next" => 2,
              "updates" => [
                %{
                  "sequence" => 1,
                  "update" => %{
                    "type" => "event",
                    "data" => %{"type" => "agent_message", "text" => "Ready when you are."}
                  }
                }
              ]
            })

          "send_message" ->
            ok("accepted", nil)

          "transcribe_audio" ->
            send(test_process, {:audio_path_exists, File.exists?(request["data"]["path"])})
            ok("audio_transcript", "Summarize the latest changes")

          operation
          when operation in [
                 "audio_recording_started",
                 "audio_recording_stopped",
                 "audio_transcription_error"
               ] ->
            ok("accepted", nil)
        end

      {:ok, Jason.encode!(response)}
    end

    Application.put_env(:styra_web, :styra_socket_path, "/deployment/styra.sock")
    Application.put_env(:styra_web, :styra_client_options, exchange: exchange, json: Jason)

    on_exit(fn ->
      Application.put_env(:styra_web, :styra_socket_path, previous_socket_path)
      Application.delete_env(:styra_web, :styra_client_options)
    end)

    :ok
  end

  test "selects a live interaction, streams updates, and sends typed input", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")
    render_async(view)

    assert_receive {:styra_socket, "/deployment/styra.sock"}
    assert has_element?(view, "#connection-status", "styra")
    refute has_element?(view, "#connection-form")
    assert has_element?(view, "#interaction-styra-1", "Review auth")

    view
    |> element("#interaction-styra-1")
    |> render_click()

    render_async(view)

    assert has_element?(view, "#update-1", "Ready when you are.")
    assert has_element?(view, "#voice-recorder")
    assert has_element?(view, "#audio-upload-form input[type=file]")

    view
    |> form("#message-form", message: %{text: "List the changed files", contract: "files"})
    |> render_submit()

    assert_receive {:styra_request,
                    %{
                      "operation" => "send_message",
                      "data" => %{
                        "id" => "styra-1",
                        "message" => %{
                          "text" => "List the changed files",
                          "contract" => "files"
                        }
                      }
                    }}

    view
    |> element("#back-to-interactions")
    |> render_click()

    refute has_element?(view, "#message-form")
    assert has_element?(view, "#interaction-styra-1")
  end

  test "transcribes a recording into the editor and waits for an explicit send", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")
    render_async(view)

    view |> element("#interaction-styra-1") |> render_click()
    render_async(view)

    view |> element("#voice-recorder") |> render_hook("audio_recording_started", %{})
    assert has_element?(view, "#voice-status", "Recording")
    assert_receive {:styra_request, %{"operation" => "audio_recording_started"}}

    view
    |> element("#voice-recorder")
    |> render_hook("audio_recording_stopped", %{
      "contract" => "files",
      "before" => "Draft: ",
      "after" => " please"
    })

    assert_receive {:styra_request, %{"operation" => "audio_recording_stopped"}}

    upload =
      file_input(view, "#audio-upload-form", :audio, [
        %{
          last_modified: 1_725_000_000_000,
          name: "voice-message.wav",
          content: wav(),
          type: "audio/wav"
        }
      ])

    render_upload(upload, "voice-message.wav")
    render_async(view)

    assert_receive {:styra_request,
                    %{
                      "operation" => "transcribe_audio",
                      "data" => %{"path" => path}
                    }}

    assert String.ends_with?(path, ".wav")
    assert_receive {:audio_path_exists, true}
    refute_receive {:styra_request, %{"operation" => "send_message"}}

    assert has_element?(
             view,
             "#message-form textarea",
             "Draft: Summarize the latest changes please"
           )

    view
    |> form("#message-form",
      message: %{
        text: "Draft: Summarize the latest changes please",
        contract: "files"
      }
    )
    |> render_submit()

    assert_receive {:styra_request,
                    %{
                      "operation" => "send_message",
                      "data" => %{
                        "id" => "styra-1",
                        "message" => %{
                          "text" => "Draft: Summarize the latest changes please",
                          "contract" => "files"
                        }
                      }
                    }}

    view |> element("#voice-recorder") |> render_hook("audio_recording_started", %{})

    view
    |> element("#voice-recorder")
    |> render_hook("audio_recording_stopped", %{
      "contract" => "none",
      "before" => "Second: ",
      "after" => ""
    })

    second_upload =
      file_input(view, "#audio-upload-form", :audio, [
        %{
          last_modified: 1_725_000_000_001,
          name: "voice-message.wav",
          content: wav(),
          type: "audio/wav"
        }
      ])

    render_upload(second_upload, "voice-message.wav")
    render_async(view)

    assert_receive {:styra_request, %{"operation" => "transcribe_audio"}}
    assert_receive {:audio_path_exists, true}
    refute_receive {:styra_request, %{"operation" => "send_message"}}
    assert has_element?(view, "#message-form textarea", "Second: Summarize the latest changes")
  end

  defp ok(type, data) do
    %{"status" => "ok", "response" => %{"type" => type, "data" => data}}
  end

  defp interaction do
    %{
      "id" => "styra-1",
      "name" => "Review auth",
      "workspace_id" => "workspace-1",
      "workspace" => "/work/project",
      "activity" => "pending",
      "last_message" => "Ready when you are.",
      "selection" => %{
        "provider" => "codex",
        "model" => "gpt-5.6-terra",
        "effort" => "medium"
      }
    }
  end

  defp wav do
    "RIFF" <>
      <<36::little-32>> <>
      "WAVEfmt " <>
      <<16::little-32, 1::little-16, 1::little-16, 16_000::little-32, 32_000::little-32,
        2::little-16, 16::little-16>> <>
      "data" <>
      <<0::little-32>>
  end
end
