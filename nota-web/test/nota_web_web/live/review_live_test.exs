defmodule NotaWebWeb.ReviewLiveTest do
  use NotaWebWeb.ConnCase, async: true

  import Phoenix.LiveViewTest

  test "lists reviews and renders notes and suggestion diffs", %{conn: conn} do
    {:ok, view, html} = live(conn, "/")

    assert html =~ "nota/sample"
    assert html =~ "Please rename this helper."
    assert html =~ "lib/demo.ex"
    assert html =~ "Invalid review branches"
    assert html =~ "nota/broken"

    html = view |> element("button", "Show 1 changed file(s)") |> render_click()

    assert html =~ "defmodule Demo do"
    assert html =~ "def extra, do: 3"
  end
end
