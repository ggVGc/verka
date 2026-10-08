defmodule NotaWebWeb.ReviewLive do
  @moduledoc """
  Browses nota review branches in a repository and renders each entry.

  The selected repository and branch live in the URL so a view can be shared
  or reloaded. Review structure comes from the `nota` CLI and the diffs come
  from git; diffs are loaded the first time a suggestion is expanded.
  """

  use NotaWebWeb, :live_view

  alias NotaWeb.{Markdown, Nota}
  import NotaWebWeb.ReviewComponents

  @impl true
  def mount(_params, _session, socket) do
    {:ok,
     assign(socket,
       page_title: "Reviews",
       repository_input: "",
       repository: nil,
       repository_root: nil,
       reviews: [],
       diagnostics: [],
       selected_branch: nil,
       review: nil,
       messages: %{},
       diffs: %{},
       sources: %{},
       expanded: MapSet.new(),
       index_error: nil,
       review_error: nil,
       repo_form: to_form(%{"path" => ""}, as: :repository)
     )}
  end

  @impl true
  def handle_params(params, _uri, socket) do
    repository = params["repository"] || socket.assigns.repository || default_repository()

    socket =
      socket
      |> assign(repo_form: to_form(%{"path" => repository}, as: :repository))
      |> load_index(repository)

    branch = params["branch"] || first_branch(socket.assigns.reviews)

    {:noreply, load_review(socket, branch)}
  end

  @impl true
  def handle_event("select_branch", %{"branch" => branch}, socket) do
    {:noreply, push_patch(socket, to: reviews_path(socket.assigns.repository_input, branch))}
  end

  def handle_event("set_repository", %{"repository" => %{"path" => path}}, socket) do
    {:noreply, push_patch(socket, to: reviews_path(String.trim(path), nil))}
  end

  def handle_event("refresh", _params, socket) do
    socket =
      socket
      |> assign(repo_form: to_form(%{"path" => socket.assigns.repository_input}, as: :repository))
      |> load_index(socket.assigns.repository)

    {:noreply, load_review(socket, socket.assigns.selected_branch)}
  end

  def handle_event("toggle_entry", %{"commit" => commit}, socket) do
    if MapSet.member?(socket.assigns.expanded, commit) do
      {:noreply, assign(socket, expanded: MapSet.delete(socket.assigns.expanded, commit))}
    else
      socket = ensure_diff(socket, commit)
      {:noreply, assign(socket, expanded: MapSet.put(socket.assigns.expanded, commit))}
    end
  end

  def handle_event("expand_all", _params, socket) do
    commits = suggestion_commits(socket.assigns.review)
    socket = Enum.reduce(commits, socket, &ensure_diff(&2, &1))
    {:noreply, assign(socket, expanded: MapSet.new(commits))}
  end

  def handle_event("collapse_all", _params, socket) do
    {:noreply, assign(socket, expanded: MapSet.new())}
  end

  defp load_index(socket, repository) do
    case Nota.client().list_reviews(repository) do
      {:ok, %{root: root, reviews: reviews, diagnostics: diagnostics}} ->
        assign(socket,
          repository: repository,
          repository_root: root,
          reviews: reviews,
          diagnostics: diagnostics,
          index_error: nil
        )

      {:error, message} ->
        assign(socket,
          repository: repository,
          repository_root: nil,
          reviews: [],
          diagnostics: [],
          index_error: message,
          review: nil,
          selected_branch: nil
        )
    end
  end

  defp load_review(socket, nil) do
    assign(socket,
      review: nil,
      selected_branch: nil,
      messages: %{},
      diffs: %{},
      sources: %{},
      expanded: MapSet.new(),
      review_error: nil
    )
  end

  defp load_review(socket, branch) do
    if socket.assigns.repository_root do
      case Nota.client().load_review(socket.assigns.repository, branch) do
        {:ok, %{review: review}} ->
          messages = Map.new(review.entries, &{&1.commit, Markdown.to_html(&1.message)})

          sources =
            review.entries
            |> Enum.filter(& &1.source)
            |> Map.new(
              &{&1.commit, Nota.client().source_lines(socket.assigns.repository, &1.source)}
            )

          assign(socket,
            review: review,
            selected_branch: branch,
            messages: messages,
            diffs: %{},
            sources: sources,
            expanded: MapSet.new(),
            review_error: nil
          )

        {:error, message} ->
          assign(socket,
            review: nil,
            selected_branch: branch,
            messages: %{},
            diffs: %{},
            sources: %{},
            expanded: MapSet.new(),
            review_error: message
          )
      end
    else
      assign(socket, review: nil, selected_branch: branch, review_error: nil)
    end
  end

  defp ensure_diff(%{assigns: %{diffs: diffs}} = socket, commit) do
    if Map.has_key?(diffs, commit) do
      socket
    else
      assign(socket,
        diffs: Map.put(diffs, commit, Nota.client().diff(socket.assigns.repository, commit))
      )
    end
  end

  defp first_branch([]), do: nil
  defp first_branch([%{branch: branch} | _rest]), do: branch

  defp suggestion_commits(nil), do: []

  defp suggestion_commits(review) do
    review.entries
    |> Enum.filter(&(&1.kind == :suggestion))
    |> Enum.map(& &1.commit)
  end

  defp reviews_path(repository, branch) do
    params = [{"repository", repository}]
    params = if branch in [nil, ""], do: params, else: params ++ [{"branch", branch}]
    "/?" <> URI.encode_query(params)
  end

  defp default_repository do
    System.get_env("NOTA_REPOSITORY") || File.cwd!()
  end
end
