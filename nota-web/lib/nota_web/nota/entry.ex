defmodule NotaWeb.Nota.Entry do
  @moduledoc "One review entry: a prose note or a suggestion commit."

  alias NotaWeb.Nota.NoteSource

  @enforce_keys [:commit, :message, :kind]
  defstruct [:commit, :message, :kind, :source, :meta, paths: []]

  @type kind :: :note | :suggestion

  @type t :: %__MODULE__{
          commit: String.t(),
          message: String.t(),
          kind: kind(),
          paths: [String.t()],
          source: NoteSource.t() | nil,
          meta: map() | nil
        }

  @spec from_json(map()) :: t()
  def from_json(json) when is_map(json) do
    %__MODULE__{
      commit: json["commit"],
      message: json["message"],
      kind: kind(json["kind"]),
      paths: json["paths"] || [],
      source: NoteSource.from_json(json["source"])
    }
  end

  @spec note?(t()) :: boolean()
  def note?(%__MODULE__{kind: :note}), do: true
  def note?(%__MODULE__{}), do: false

  @spec suggestion?(t()) :: boolean()
  def suggestion?(%__MODULE__{kind: :suggestion}), do: true
  def suggestion?(%__MODULE__{}), do: false

  defp kind("note"), do: :note
  defp kind("suggestion"), do: :suggestion
end
