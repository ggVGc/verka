defmodule NotaWeb.FakeNota do
  @moduledoc "An in-memory `NotaWeb.Nota` for tests."

  alias NotaWeb.Nota.{Diff, Entry, NoteSource, Review, Summary}

  @diff_patch "diff --git a/lib/demo.ex b/lib/demo.ex\nindex 1111111..2222222 100644\n--- a/lib/demo.ex\n+++ b/lib/demo.ex\n@@ -1,3 +1,4 @@ defmodule Demo do\n defmodule Demo do\n-  def old, do: 1\n+  def new, do: 2\n+  def extra, do: 3\n end\n"

  def list_reviews(repository) do
    {:ok, %{root: repository, reviews: [summary()], diagnostics: [diagnostic()]}}
  end

  def load_review(_repository, "nota/sample") do
    {:ok, %{root: "/repo", review: review()}}
  end

  def load_review(_repository, _branch), do: {:error, "review not found"}

  def diff(_repository, "suggestion123"), do: {:ok, Diff.parse(@diff_patch)}
  def diff(_repository, _commit), do: {:error, "no such commit"}

  def source_lines(_repository, %NoteSource{} = source) do
    {:ok,
     Enum.map(source.first..source.last, fn number ->
       %{number: number, text: "  line #{number} of the pinned file"}
     end)}
  end

  defp summary do
    %Summary{
      branch: "nota/sample",
      marker: "marker123",
      subject: "subject1234567890",
      tip: "tip123",
      notes: 1,
      suggestions: 1
    }
  end

  defp diagnostic do
    %NotaWeb.Nota.Diagnostic{branch: "nota/broken", message: "marker has no review trailer"}
  end

  defp review do
    %Review{
      branch: "nota/sample",
      marker: "marker123",
      subject: "subject1234567890",
      subject_meta: meta("subject1234567890", "Add review viewer"),
      entries: [
        %Entry{
          commit: "note123",
          message: "Please rename this helper.",
          kind: :note,
          paths: [],
          source: %NoteSource{
            revision: "subject1234567890",
            path: "lib/demo.ex",
            first: 2,
            last: 3
          },
          meta: meta("note123", "Please rename this helper.")
        },
        %Entry{
          commit: "suggestion123",
          message: "**Rename** the helper to `greet/1`.",
          kind: :suggestion,
          paths: ["lib/demo.ex"],
          source: nil,
          meta: meta("suggestion123", "Rename the helper")
        }
      ]
    }
  end

  defp meta(sha, subject) do
    %{
      sha: sha,
      short: String.slice(sha, 0, 12),
      author: "Test Author",
      date: "2026-10-07T12:00:00Z",
      subject: subject
    }
  end
end
