defmodule NotaWeb.Nota.NoteSource do
  @moduledoc "The file lines a note is anchored to, in one commit's version."

  @enforce_keys [:revision, :path, :first, :last]
  defstruct [:revision, :path, :first, :last]

  @type t :: %__MODULE__{
          revision: String.t(),
          path: String.t(),
          first: pos_integer(),
          last: pos_integer()
        }

  @spec from_json(map() | nil) :: t() | nil
  def from_json(nil), do: nil

  def from_json(json) when is_map(json) do
    %__MODULE__{
      revision: json["revision"],
      path: json["path"],
      first: json["first"],
      last: json["last"]
    }
  end
end
