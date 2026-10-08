defmodule NotaWeb.Nota do
  @moduledoc """
  Reads nota review branches for the web application.

  Review structure and validation come from the `nota` CLI's JSON contract;
  git supplies the diffs, commit metadata, and file lines the viewer shows.
  Set the `:nota_client` application setting to substitute a fake in tests.
  """

  alias NotaWeb.Nota.{Cli, Diagnostic, Git, Review, Summary}

  @type repository :: String.t()

  @spec client() :: module()
  def client, do: Application.get_env(:nota_web, :nota_client) || __MODULE__

  @spec list_reviews(repository()) ::
          {:ok, %{root: String.t(), reviews: [Summary.t()], diagnostics: [Diagnostic.t()]}}
          | {:error, String.t()}
  def list_reviews(repository) do
    with {:ok, root} <- Git.repository_root(repository),
         {:ok, %{"reviews" => reviews, "diagnostics" => diagnostics}} <- Cli.list(root) do
      {:ok,
       %{
         root: root,
         reviews: Enum.map(reviews, &Summary.from_json/1),
         diagnostics: Enum.map(diagnostics, &Diagnostic.from_json/1)
       }}
    end
  end

  @spec load_review(repository(), String.t()) ::
          {:ok, %{root: String.t(), review: Review.t()}} | {:error, String.t()}
  def load_review(repository, branch) do
    with {:ok, root} <- Git.repository_root(repository),
         {:ok, raw} <- Cli.show(root, branch) do
      review = Review.from_json(raw)
      meta = review_meta(root, review)

      review = %{
        review
        | subject_meta: Map.get(meta, review.subject),
          entries: Enum.map(review.entries, &%{&1 | meta: Map.get(meta, &1.commit)})
      }

      {:ok, %{root: root, review: review}}
    end
  end

  @spec diff(repository(), String.t()) :: {:ok, [map()]} | {:error, String.t()}
  def diff(repository, commit), do: Git.diff(repository, commit)

  @spec source_lines(repository(), NotaWeb.Nota.NoteSource.t()) ::
          {:ok, [%{number: pos_integer(), text: String.t()}]} | {:error, String.t()}
  def source_lines(repository, source), do: Git.source_lines(repository, source)

  @spec changed_files(Review.t()) :: [{String.t(), pos_integer()}]
  def changed_files(%Review{} = review) do
    review.entries
    |> Enum.filter(&(&1.kind == :suggestion))
    |> Enum.flat_map(& &1.paths)
    |> Enum.frequencies()
    |> Enum.sort_by(fn {path, _count} -> path end)
  end

  defp review_meta(root, review) do
    commits = [review.subject | Enum.map(review.entries, & &1.commit)]

    case Git.commit_meta(root, commits) do
      {:ok, meta} -> meta
      {:error, _message} -> %{}
    end
  end
end
