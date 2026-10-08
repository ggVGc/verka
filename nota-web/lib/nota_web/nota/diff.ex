defmodule NotaWeb.Nota.Diff do
  @moduledoc """
  Parses a unified diff into files, hunks, and individually numbered lines.

  The shapes returned are plain maps:

      %{
        path: "lib/foo.ex",
        old_path: "lib/foo.ex",
        status: :modified | :added | :deleted | :renamed,
        binary: boolean(),
        additions: non_neg_integer(),
        deletions: non_neg_integer(),
        hunks: [
          %{
            header: "@@ -1,3 +1,4 @@",
            context: "def foo",
            old_start: 1,
            new_start: 1,
            lines: [%{kind: :add | :del | :ctx | :meta, text: String.t(),
                      old_no: integer() | nil, new_no: integer() | nil}]
          }
        ]
      }
  """

  @hunk_header ~r/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/

  @type line :: %{
          kind: :add | :del | :ctx | :meta,
          text: String.t(),
          old_no: pos_integer() | nil,
          new_no: pos_integer() | nil
        }

  @spec parse(String.t()) :: [map()]
  def parse(patch) when is_binary(patch) do
    patch
    |> String.split("\n")
    |> Enum.reduce(%{files: [], file: nil, hunk: nil}, &step/2)
    |> flush()
  end

  defp step(line, state) do
    cond do
      String.starts_with?(line, "diff --git ") ->
        state |> close_hunk() |> close_file() |> start_file(line)

      state.file == nil ->
        state

      String.starts_with?(line, "new file mode") ->
        put_file(state, :status, :added)

      String.starts_with?(line, "deleted file mode") ->
        put_file(state, :status, :deleted)

      String.starts_with?(line, "rename from ") ->
        state
        |> put_file(:old_path, String.replace_prefix(line, "rename from ", ""))
        |> put_file(:status, :renamed)

      String.starts_with?(line, "rename to ") ->
        put_file(state, :path, String.replace_prefix(line, "rename to ", ""))

      String.starts_with?(line, "copy from ") ->
        put_file(state, :old_path, String.replace_prefix(line, "copy from ", ""))

      String.starts_with?(line, "copy to ") ->
        put_file(state, :path, String.replace_prefix(line, "copy to ", ""))

      String.starts_with?(line, "--- ") ->
        put_file(state, :old_path, diff_path(line, "a/"))

      String.starts_with?(line, "+++ ") ->
        put_file(state, :path, diff_path(line, "b/"))

      String.starts_with?(line, "Binary files ") or String.starts_with?(line, "GIT binary patch") ->
        put_file(state, :binary, true)

      header = hunk_header(line) ->
        state |> close_hunk() |> start_hunk(header)

      state.hunk != nil ->
        hunk_line(state, line)

      true ->
        state
    end
  end

  defp start_file(state, line) do
    {old_path, path} = header_paths(line)

    %{
      state
      | file: %{
          path: path,
          old_path: old_path,
          status: :modified,
          binary: false,
          additions: 0,
          deletions: 0,
          hunks: []
        }
    }
  end

  defp header_paths(line) do
    case Regex.run(~r/^diff --git a\/(.*) b\/(.*)$/, line) do
      [_, old_path, path] -> {old_path, path}
      _ -> {nil, nil}
    end
  end

  defp close_file(%{file: nil} = state), do: state

  defp close_file(%{files: files, file: file} = state) do
    file = %{file | path: file.path || file.old_path, hunks: Enum.reverse(file.hunks)}
    %{state | files: [file | files], file: nil}
  end

  defp put_file(%{file: file} = state, key, value), do: %{state | file: Map.put(file, key, value)}

  defp hunk_header(line) do
    case Regex.run(@hunk_header, line) do
      [_, old, old_count, new, new_count, context] ->
        %{
          header: line,
          context: String.trim(context),
          old_start: String.to_integer(old),
          old_count: parse_count(old_count),
          new_start: String.to_integer(new),
          new_count: parse_count(new_count)
        }

      _ ->
        nil
    end
  end

  defp parse_count(nil), do: 1
  defp parse_count(""), do: 1
  defp parse_count(count), do: String.to_integer(count)

  defp start_hunk(state, header) do
    hunk = %{
      header: header.header,
      context: header.context,
      old_start: header.old_start,
      old_count: header.old_count,
      new_start: header.new_start,
      new_count: header.new_count,
      old_no: header.old_start,
      new_no: header.new_start,
      lines: []
    }

    %{state | hunk: hunk}
  end

  defp close_hunk(%{hunk: nil} = state), do: state

  defp close_hunk(%{hunk: hunk, file: file} = state) do
    hunk = %{hunk | lines: Enum.reverse(hunk.lines)}

    file = %{
      file
      | hunks: [hunk | file.hunks],
        additions: file.additions + count(hunk.lines, :add),
        deletions: file.deletions + count(hunk.lines, :del)
    }

    %{state | hunk: nil, file: file}
  end

  defp count(lines, kind), do: Enum.count(lines, &(&1.kind == kind))

  defp hunk_line(state, "+" <> text), do: push_line(state, :add, text)
  defp hunk_line(state, "-" <> text), do: push_line(state, :del, text)
  defp hunk_line(state, " " <> text), do: push_line(state, :ctx, text)
  defp hunk_line(state, "\\" <> _ = text), do: push_line(state, :meta, text)
  defp hunk_line(state, _line), do: state

  defp push_line(%{hunk: hunk} = state, :add, text) do
    line = %{kind: :add, text: text, old_no: nil, new_no: hunk.new_no}
    %{state | hunk: %{hunk | lines: [line | hunk.lines], new_no: hunk.new_no + 1}}
  end

  defp push_line(%{hunk: hunk} = state, :del, text) do
    line = %{kind: :del, text: text, old_no: hunk.old_no, new_no: nil}
    %{state | hunk: %{hunk | lines: [line | hunk.lines], old_no: hunk.old_no + 1}}
  end

  defp push_line(%{hunk: hunk} = state, :ctx, text) do
    line = %{kind: :ctx, text: text, old_no: hunk.old_no, new_no: hunk.new_no}

    %{
      state
      | hunk: %{
          hunk
          | lines: [line | hunk.lines],
            old_no: hunk.old_no + 1,
            new_no: hunk.new_no + 1
        }
    }
  end

  defp push_line(%{hunk: hunk} = state, :meta, text) do
    line = %{kind: :meta, text: text, old_no: nil, new_no: nil}
    %{state | hunk: %{hunk | lines: [line | hunk.lines]}}
  end

  defp flush(state) do
    state |> close_hunk() |> close_file() |> Map.fetch!(:files) |> Enum.reverse()
  end

  defp diff_path(line, git_prefix) do
    value =
      line
      |> String.replace_prefix("--- ", "")
      |> String.replace_prefix("+++ ", "")
      |> String.split("\t")
      |> List.first()
      |> String.trim()

    cond do
      value in [nil, "", "/dev/null"] -> nil
      String.starts_with?(value, git_prefix) -> String.replace_prefix(value, git_prefix, "")
      true -> value
    end
  end
end
