defmodule NotaWeb.Nota.Git do
  @moduledoc """
  The git commands the review viewer needs.

  Everything runs with `git -C <repository> ...` so the viewer never touches
  the working tree, index, or checkout. Errors are returned as readable
  strings rather than raised so a malformed repository cannot crash a
  LiveView.
  """

  alias NotaWeb.Nota.{Diff, NoteSource}

  @meta_format "%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e"

  @type commit_meta :: %{
          sha: String.t(),
          short: String.t(),
          author: String.t(),
          date: String.t() | nil,
          subject: String.t()
        }

  @spec repository_root(String.t()) :: {:ok, String.t()} | {:error, String.t()}
  def repository_root(path) when is_binary(path) do
    case run(["-C", path, "rev-parse", "--show-toplevel"]) do
      {:ok, output} -> {:ok, String.trim(output)}
      {:error, message} -> {:error, "not a Git repository: #{message}"}
    end
  end

  @spec commit_meta(String.t(), [String.t()]) ::
          {:ok, %{String.t() => commit_meta()}} | {:error, String.t()}
  def commit_meta(_repository, []), do: {:ok, %{}}

  def commit_meta(repository, commits) when is_list(commits) do
    args = ["-C", repository, "log", "--no-walk", "--format=#{@meta_format}" | commits]

    case run(args) do
      {:ok, output} -> {:ok, parse_meta(output)}
      {:error, message} -> {:error, message}
    end
  end

  @spec diff(String.t(), String.t()) :: {:ok, [map()]} | {:error, String.t()}
  def diff(repository, commit) do
    args = [
      "-C",
      repository,
      "show",
      "--no-color",
      "--format=",
      "--patch",
      "--unified=3",
      commit
    ]

    case run(args) do
      {:ok, output} -> {:ok, Diff.parse(output)}
      {:error, message} -> {:error, message}
    end
  end

  @spec source_lines(String.t(), NoteSource.t()) ::
          {:ok, [%{number: pos_integer(), text: String.t()}]} | {:error, String.t()}
  def source_lines(repository, %NoteSource{} = source) do
    object = "#{source.revision}:#{source.path}"

    case run(["-C", repository, "show", object]) do
      {:ok, content} ->
        {:ok, numbered_lines(content, source.first, source.last)}

      {:error, message} ->
        {:error, "could not read #{source.path} at #{short(source.revision)}: #{message}"}
    end
  end

  @spec short(String.t()) :: String.t()
  def short(sha) when is_binary(sha), do: String.slice(sha, 0, 12)

  defp numbered_lines(content, first, last) do
    lines = String.split(content, "\n")
    lines = if List.last(lines) == "", do: Enum.drop(lines, -1), else: lines

    last = min(last, length(lines))

    if first > last or first < 1 do
      []
    else
      lines
      |> Enum.slice((first - 1)..(last - 1))
      |> Enum.with_index(first)
      |> Enum.map(fn {text, number} -> %{number: number, text: text} end)
    end
  end

  defp parse_meta(output) do
    output
    |> String.split("\x1e")
    |> Enum.map(&String.trim/1)
    |> Enum.reject(&(&1 == ""))
    |> Map.new(fn record ->
      case String.split(record, "\x1f") do
        [sha, short, author, date, subject] ->
          {sha, %{sha: sha, short: short, author: author, date: date, subject: subject}}

        _ ->
          {record, %{sha: record, short: short(record), author: "", date: nil, subject: ""}}
      end
    end)
  end

  defp run(args) do
    case System.cmd("git", args, stderr_to_stdout: true) do
      {output, 0} ->
        {:ok, output}

      {output, code} ->
        {:error, error_message(output, "git exited with status #{code}")}
    end
  rescue
    ErlangError -> {:error, "git is not available on PATH"}
  end

  defp error_message(output, fallback) do
    case String.trim(output) do
      "" -> fallback
      message -> message
    end
  end
end
