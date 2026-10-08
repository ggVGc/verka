defmodule NotaWeb.Nota.Review do
  @moduledoc "A loaded review branch with its entries in review order."

  alias NotaWeb.Nota.Entry

  @enforce_keys [:branch, :marker, :subject]
  defstruct [:branch, :marker, :subject, :subject_meta, entries: []]

  @type t :: %__MODULE__{
          branch: String.t(),
          marker: String.t(),
          subject: String.t(),
          subject_meta: map() | nil,
          entries: [Entry.t()]
        }

  @spec from_json(map()) :: t()
  def from_json(json) when is_map(json) do
    %__MODULE__{
      branch: json["branch"],
      marker: json["marker"],
      subject: json["subject"],
      entries: Enum.map(json["entries"] || [], &Entry.from_json/1)
    }
  end
end
