defmodule NotaWeb.Markdown do
  @moduledoc "Renders a note or suggestion message as HTML."

  @spec to_html(String.t() | nil) :: String.t()
  def to_html(nil), do: ""

  def to_html(text) when is_binary(text) do
    case Earmark.as_html(text, %Earmark.Options{smartypants: false}) do
      {:ok, html, _warnings} -> html
      {:error, html, _warnings} -> html
    end
  end
end
