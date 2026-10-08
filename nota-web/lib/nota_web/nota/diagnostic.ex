defmodule NotaWeb.Nota.Diagnostic do
  @moduledoc "A branch that looks like a review but failed validation."

  @enforce_keys [:branch, :message]
  defstruct [:branch, :message]

  @type t :: %__MODULE__{branch: String.t(), message: String.t()}

  @spec from_json(map()) :: t()
  def from_json(json) when is_map(json) do
    %__MODULE__{branch: json["branch"], message: json["message"]}
  end
end
