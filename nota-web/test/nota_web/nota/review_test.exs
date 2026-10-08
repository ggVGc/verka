defmodule NotaWeb.Nota.ReviewTest do
  use ExUnit.Case, async: true

  alias NotaWeb.Nota.Review

  test "parses a review with a line-anchored note and a suggestion" do
    review =
      Review.from_json(%{
        "branch" => "nota/sample",
        "marker" => "marker",
        "subject" => "subject",
        "entries" => [
          %{
            "commit" => "c1",
            "message" => "hello",
            "kind" => "note",
            "paths" => [],
            "source" => %{
              "revision" => "subject",
              "path" => "lib/foo.ex",
              "first" => 2,
              "last" => 3
            }
          },
          %{
            "commit" => "c2",
            "message" => "change it",
            "kind" => "suggestion",
            "paths" => ["lib/foo.ex"],
            "source" => nil
          }
        ]
      })

    assert review.branch == "nota/sample"
    assert [note, suggestion] = review.entries
    assert note.kind == :note
    assert note.source.path == "lib/foo.ex"
    assert note.source.first == 2
    assert suggestion.kind == :suggestion
    assert suggestion.paths == ["lib/foo.ex"]
    assert suggestion.source == nil
  end
end
