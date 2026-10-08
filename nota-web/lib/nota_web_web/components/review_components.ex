defmodule NotaWebWeb.ReviewComponents do
  @moduledoc "Presentational components for review notes and suggestion diffs."

  use NotaWebWeb, :html

  @doc "The pinned subject, counts, and changed-file summary for a review."
  attr :review, :map, required: true
  attr :changed_files, :list, required: true

  def review_header(assigns) do
    ~H"""
    <div class="rounded-2xl border border-slate-200 bg-white p-5 shadow-sm">
      <div class="flex flex-wrap items-start justify-between gap-4">
        <div class="min-w-0">
          <h2 class="truncate font-mono text-lg font-semibold tracking-tight text-slate-900">
            {@review.branch}
          </h2>
          <p class="mt-1 flex flex-wrap items-center gap-x-1.5 text-sm text-slate-500">
            <span>pinned subject</span>
            <span class="font-mono text-slate-700">
              {NotaWeb.Nota.Git.short(@review.subject)}
            </span>
            <%= if @review.subject_meta do %>
              <span class="text-slate-300">·</span>
              <span class="truncate">{@review.subject_meta.subject}</span>
              <span class="text-slate-300">·</span>
              <span>{@review.subject_meta.author}</span>
              <span class="text-slate-300">·</span>
              <span>{format_date(@review.subject_meta.date)}</span>
            <% end %>
          </p>
        </div>
        <div class="flex shrink-0 gap-2">
          <.stat label="entries" value={length(@review.entries)} />
          <.stat label="notes" value={count_kind(@review.entries, :note)} tone={:sky} />
          <.stat
            label="suggestions"
            value={count_kind(@review.entries, :suggestion)}
            tone={:violet}
          />
        </div>
      </div>

      <%= if @changed_files != [] do %>
        <div class="mt-4 border-t border-slate-100 pt-4">
          <p class="text-xs font-semibold tracking-wide text-slate-400 uppercase">Files changed</p>
          <div class="mt-2 flex flex-wrap gap-1.5">
            <%= for {path, count} <- @changed_files do %>
              <span
                class="inline-flex items-center gap-1 rounded-md bg-slate-100 px-2 py-0.5 font-mono text-[11px] text-slate-600"
                title={"#{count} suggestion(s) touch this file"}
              >
                <.icon name="hero-document" class="size-3 text-slate-400" />
                {path}
                <%= if count > 1 do %>
                  <span class="text-slate-400">×{count}</span>
                <% end %>
              </span>
            <% end %>
          </div>
        </div>
      <% end %>
    </div>
    """
  end

  attr :label, :string, required: true
  attr :value, :integer, required: true
  attr :tone, :atom, default: :slate, values: [:slate, :sky, :violet]

  defp stat(assigns) do
    ~H"""
    <span class={[
      "inline-flex flex-col items-center rounded-xl px-3 py-1.5",
      tone_classes(@tone)
    ]}>
      <span class="text-base leading-tight font-semibold">{@value}</span>
      <span class="text-[10px] tracking-wide uppercase opacity-70">{@label}</span>
    </span>
    """
  end

  @doc "One note or suggestion entry."
  attr :entry, :map, required: true
  attr :html, :string, required: true
  attr :expanded, :boolean, required: true
  attr :diff, :any, default: nil
  attr :source, :any, default: nil

  def entry_card(%{entry: %{kind: :note}} = assigns) do
    ~H"""
    <article
      id={"entry-" <> @entry.commit}
      class="rounded-2xl border border-slate-200 bg-white shadow-sm transition hover:shadow-md"
    >
      <div class="flex items-start gap-3 p-4">
        <span class="mt-0.5 grid size-8 shrink-0 place-items-center rounded-full bg-sky-100 text-sky-700">
          <.icon name="hero-chat-bubble-left-right" class="size-4" />
        </span>
        <div class="min-w-0 flex-1">
          <.entry_meta
            kind="Note"
            kind_class="text-sky-700"
            meta={@entry.meta}
            commit={@entry.commit}
          />
          <div class="note-prose mt-2">{raw(@html)}</div>
          <%= if @entry.source do %>
            <.note_source source={@entry.source} result={@source} />
          <% end %>
        </div>
      </div>
    </article>
    """
  end

  def entry_card(%{entry: %{kind: :suggestion}} = assigns) do
    ~H"""
    <article
      id={"entry-" <> @entry.commit}
      class="rounded-2xl border border-slate-200 bg-white shadow-sm transition hover:shadow-md"
    >
      <div class="flex items-start gap-3 p-4">
        <span class="mt-0.5 grid size-8 shrink-0 place-items-center rounded-full bg-violet-100 text-violet-700">
          <.icon name="hero-code-bracket-square" class="size-4" />
        </span>
        <div class="min-w-0 flex-1">
          <.entry_meta
            kind="Suggestion"
            kind_class="text-violet-700"
            meta={@entry.meta}
            commit={@entry.commit}
          />
          <div class="note-prose mt-2">{raw(@html)}</div>
          <button
            type="button"
            phx-click="toggle_entry"
            phx-value-commit={@entry.commit}
            class="mt-3 inline-flex items-center gap-1.5 rounded-lg border border-slate-200 bg-slate-50 px-2.5 py-1.5 text-xs font-medium text-slate-600 transition hover:border-slate-300 hover:bg-white"
          >
            <.icon
              name="hero-chevron-right"
              class={["size-3.5 transition-transform", @expanded && "rotate-90"]}
            />
            <%= if @expanded do %>
              Hide changes
            <% else %>
              Show {length(@entry.paths)} changed file(s)
            <% end %>
          </button>
          <%= if @expanded do %>
            <.diff_panel result={@diff} />
          <% end %>
        </div>
      </div>
    </article>
    """
  end

  attr :kind, :string, required: true
  attr :kind_class, :string, required: true
  attr :meta, :map, default: nil
  attr :commit, :string, required: true

  defp entry_meta(assigns) do
    ~H"""
    <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-slate-500">
      <span class={["font-semibold", @kind_class]}>{@kind}</span>
      <span class="text-slate-300">•</span>
      <span class="font-mono">{NotaWeb.Nota.Git.short(@commit)}</span>
      <%= if @meta do %>
        <span class="text-slate-300">•</span>
        <span>{@meta.author}</span>
        <span class="text-slate-300">·</span>
        <span>{format_date(@meta.date)}</span>
      <% end %>
    </div>
    """
  end

  attr :source, :map, required: true
  attr :result, :any, default: nil

  defp note_source(assigns) do
    ~H"""
    <div class="mt-3 overflow-hidden rounded-xl border border-slate-200 bg-slate-50/60">
      <div class="flex items-center gap-1.5 border-b border-slate-200 px-3 py-1.5 font-mono text-[11px] text-slate-500">
        <.icon name="hero-map-pin" class="size-3.5 text-slate-400" />
        <span class="truncate">{@source.path}</span>
        <span class="text-slate-300">·</span>
        <span>lines {@source.first}–{@source.last}</span>
        <span class="ml-auto text-slate-400">{NotaWeb.Nota.Git.short(@source.revision)}</span>
      </div>
      <%= case @result do %>
        <% {:ok, lines} -> %>
          <div class="overflow-x-auto">
            <table class="diff-table w-full border-collapse font-mono text-[12.5px] leading-5">
              <tbody>
                <%= for line <- lines do %>
                  <tr class="bg-indigo-50/40">
                    <td class="diff-no">{line.number}</td>
                    <td class="diff-text text-slate-800 whitespace-pre">{line.text}</td>
                  </tr>
                <% end %>
              </tbody>
            </table>
          </div>
        <% {:error, message} -> %>
          <p class="px-3 py-2 text-sm text-rose-600">{message}</p>
        <% nil -> %>
          <p class="px-3 py-2 text-sm text-slate-400">Loading lines…</p>
      <% end %>
    </div>
    """
  end

  @doc "The parsed diff for one suggestion commit."
  attr :result, :any, required: true

  def diff_panel(assigns) do
    ~H"""
    <div class="mt-3">
      <%= case @result do %>
        <% {:ok, files} -> %>
          <div class="space-y-3">
            <%= if files == [] do %>
              <p class="rounded-lg bg-slate-50 p-3 text-sm text-slate-500">
                This commit changes no textual files.
              </p>
            <% end %>
            <%= for file <- files do %>
              <.diff_file file={file} />
            <% end %>
          </div>
        <% {:error, message} -> %>
          <p class="rounded-lg bg-rose-50 p-3 text-sm text-rose-700">{message}</p>
        <% nil -> %>
          <p class="rounded-lg bg-slate-50 p-3 text-sm text-slate-400">
            <.icon name="hero-arrow-path" class="mr-1 inline size-3.5 animate-spin" /> Loading diff…
          </p>
      <% end %>
    </div>
    """
  end

  attr :file, :map, required: true

  defp diff_file(assigns) do
    ~H"""
    <div class="overflow-hidden rounded-xl border border-slate-200">
      <div class="flex items-center gap-2 border-b border-slate-200 bg-slate-50 px-3 py-2">
        <span class={[
          "rounded px-1.5 py-0.5 text-[10px] font-semibold tracking-wide uppercase",
          status_class(@file.status)
        ]}>
          {status_label(@file.status)}
        </span>
        <span class="truncate font-mono text-xs text-slate-700">
          <%= if @file.old_path && @file.old_path != @file.path do %>
            <span class="text-slate-400">{@file.old_path} → </span>
          <% end %>
          {@file.path}
        </span>
        <span class="ml-auto flex shrink-0 items-center gap-2 font-mono text-[11px]">
          <span class="text-emerald-600">+{@file.additions}</span>
          <span class="text-rose-600">-{@file.deletions}</span>
        </span>
      </div>
      <%= if @file.binary do %>
        <p class="px-3 py-3 text-sm text-slate-500 italic">Binary file not shown.</p>
      <% else %>
        <div class="overflow-x-auto">
          <table class="diff-table w-full border-collapse font-mono text-[12.5px] leading-5">
            <tbody>
              <%= for hunk <- @file.hunks do %>
                <tr>
                  <td
                    colspan="3"
                    class="border-y border-slate-200 bg-slate-100 px-3 py-1 text-[11px] text-slate-500"
                  >
                    {hunk.header}
                  </td>
                </tr>
                <%= for line <- hunk.lines do %>
                  <tr class={line_row_class(line.kind)}>
                    <td class="diff-no">{line.old_no}</td>
                    <td class="diff-no">{line.new_no}</td>
                    <td class={["diff-text whitespace-pre", line_text_class(line.kind)]}>
                      <span class={["mr-1 inline-block w-3 select-none", line_text_class(line.kind)]}>
                        {line_marker(line.kind)}
                      </span>{line.text}
                    </td>
                  </tr>
                <% end %>
              <% end %>
            </tbody>
          </table>
        </div>
      <% end %>
    </div>
    """
  end

  @doc "Shown when a repository has no review branches."
  def empty_reviews(assigns) do
    ~H"""
    <div class="rounded-2xl border border-dashed border-slate-300 bg-white/60 p-6 text-center">
      <.icon name="hero-inbox" class="mx-auto size-6 text-slate-400" />
      <p class="mt-2 text-sm font-medium text-slate-700">No reviews here yet</p>
      <p class="mt-1 text-xs text-slate-500">
        Start one with <code class="rounded bg-slate-100 px-1 py-0.5 font-mono">nota start &lt;revision&gt;</code>.
      </p>
    </div>
    """
  end

  defp count_kind(entries, kind), do: Enum.count(entries, &(&1.kind == kind))

  defp tone_classes(:slate), do: "bg-slate-100 text-slate-700"
  defp tone_classes(:sky), do: "bg-sky-100 text-sky-700"
  defp tone_classes(:violet), do: "bg-violet-100 text-violet-700"

  defp status_label(:added), do: "added"
  defp status_label(:deleted), do: "deleted"
  defp status_label(:renamed), do: "renamed"
  defp status_label(_), do: "modified"

  defp status_class(:added), do: "bg-emerald-100 text-emerald-700"
  defp status_class(:deleted), do: "bg-rose-100 text-rose-700"
  defp status_class(:renamed), do: "bg-amber-100 text-amber-700"
  defp status_class(_), do: "bg-slate-200 text-slate-600"

  defp line_row_class(:add), do: "bg-emerald-50"
  defp line_row_class(:del), do: "bg-rose-50"
  defp line_row_class(:meta), do: "bg-slate-50"
  defp line_row_class(_), do: ""

  defp line_text_class(:add), do: "text-emerald-800"
  defp line_text_class(:del), do: "text-rose-800"
  defp line_text_class(:meta), do: "text-slate-400 italic"
  defp line_text_class(_), do: "text-slate-700"

  defp line_marker(:add), do: "+"
  defp line_marker(:del), do: "-"
  defp line_marker(:ctx), do: " "
  defp line_marker(_), do: ""

  defp format_date(nil), do: ""

  defp format_date(raw) when is_binary(raw) do
    case DateTime.from_iso8601(raw) do
      {:ok, datetime, _offset} -> Calendar.strftime(datetime, "%b %d, %Y %H:%M")
      _ -> raw
    end
  end
end
