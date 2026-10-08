defmodule NotaWeb.Nota.Summary do
  @moduledoc "A review branch as returned by `nota list --json`."

  @enforce_keys [:branch, :marker, :subject]
  defstruct [:branch, :marker, :subject, :tip, notes: 0, suggestions: 0]

  @type t :: %__MODULE__{
          branch: String.t(),
          marker: String.t(),
          subject: String.t(),
          tip: String.t() | nil,
          notes: non_neg_integer(),
          suggestions: non_neg_integer()
        }

  @spec from_json(map()) :: t()
  def from_json(json) when is_map(json) do
    %__MODULE__{
      branch: json["branch"],
      marker: json["marker"],
      subject: json["subject"],
      tip: json["tip"],
      notes: json["notes"] || 0,
      suggestions: json["suggestions"] || 0
    }
  end
end
