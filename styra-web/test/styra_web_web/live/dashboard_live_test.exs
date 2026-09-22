defmodule StyraWebWeb.DashboardLiveTest do
  use StyraWebWeb.ConnCase, async: false

  import Phoenix.LiveViewTest

  setup do
    test_process = self()

    exchange = fn _path, json, _timeout ->
      request = Jason.decode!(json)
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
        end

      {:ok, Jason.encode!(response)}
    end

    Application.put_env(:styra_web, :styra_client_options, exchange: exchange, json: Jason)

    on_exit(fn -> Application.delete_env(:styra_web, :styra_client_options) end)

    :ok
  end

  test "selects a live interaction, streams updates, and sends typed input", %{conn: conn} do
    {:ok, view, _html} = live(conn, "/")
    render_async(view)

    assert has_element?(view, "#interaction-styra-1", "Review auth")

    view
    |> element("#interaction-styra-1")
    |> render_click()

    render_async(view)

    assert has_element?(view, "#update-1", "Ready when you are.")

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
end
