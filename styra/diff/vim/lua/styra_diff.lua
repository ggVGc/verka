-- The code in a diff highlighted in its file's language and tinted as
-- styra-diff tints it, for Neovim. The `styra-diff` colorscheme calls this.
--
-- This follows `diff_body_lines`, `flush` and `tinted` in styra-ui's diff.rs:
-- each hunk's code is highlighted as one block, removed and added lines
-- interleaved, in the language its file's path names. Then each row's colors
-- are changed by its kind:
--
--   added    syntax colors as they are
--   removed  faded 3 tenths toward NEAR_BLACK, then 4 toward RED_FG
--   context  faded 4 tenths toward NEAR_BLACK
--
-- styra-diff highlights with TextMate grammars; this uses tree-sitter, whose
-- capture groups the colorscheme maps to the same colors. Code in a language
-- with no parser installed keeps the colorscheme's plain colors, as code in a
-- language styra-diff has no grammar for keeps its plain rendering.
--
-- `side_by_side` does the same for windows in diff mode, where each side is
-- a file of its own, highlighted however its window highlights it.

local M = {}

-- Not `styra_diff`: Neovim keeps a sign group in the namespace of its name,
-- and the colorscheme's bands are signs in that group.
local ns = vim.api.nvim_create_namespace('styra_diff_tint')

-- palette::NEAR_BLACK and palette::RED_FG.
local NEAR_BLACK = { 18, 18, 18 }
local RED_FG = { 255, 140, 140 }

local function rgb(color)
  return { math.floor(color / 65536), math.floor(color / 256) % 256, color % 256 }
end

-- `blended_style` in event_list.rs: `tenths` of the way toward `to`, rounded
-- down as its integer arithmetic does.
local function blend(c, to, tenths)
  local out = {}
  for i = 1, 3 do
    out[i] = math.floor((c[i] * (10 - tenths) + to[i] * tenths) / 10)
  end
  return out
end

local function tint(c, kind)
  if kind == '+' then
    return c
  elseif kind == '-' then
    return blend(blend(c, NEAR_BLACK, 3), RED_FG, 4)
  end
  return blend(c, NEAR_BLACK, 4)
end

-- One highlight group per color drawn, made when first needed.
local groups = {}

local function group(c)
  local hex = string.format('%02X%02X%02X', c[1], c[2], c[3])
  local name = 'StyraDiffFg' .. hex
  if not groups[name] then
    vim.api.nvim_set_hl(0, name, { fg = '#' .. hex })
    groups[name] = true
  end
  return name
end

-- A capture's foreground, falling back from `@a.b.lang` to `@a`, as
-- Neovim's own highlighter does. `nil` for a capture with none.
local function capture_fg(capture, lang)
  local name = capture .. '.' .. lang
  while true do
    local fg = vim.api.nvim_get_hl(0, { name = '@' .. name, link = false }).fg
    if fg then
      return fg
    end
    local shorter = name:match('^(.*)%.[^.]*$')
    if not shorter then
      return nil
    end
    name = shorter
  end
end

-- The tree-sitter language for `path`, if a parser for it is installed.
local function language_of(path)
  local filetype = vim.filetype.match({ filename = path })
  local lang = filetype and vim.treesitter.language.get_lang(filetype)
  if not lang then
    return nil
  end
  local ok, added = pcall(vim.treesitter.language.add, lang)
  return ok and added and lang or nil
end

-- The captures of `parser`'s trees over `source`, a string or a buffer,
-- from row `start` up to `stop`, in the order Neovim layers them: by
-- priority, then by order.
local function captures_of(parser, source, start, stop)
  local captures = {}
  parser:for_each_tree(function(tree, ltree)
    local query = vim.treesitter.query.get(ltree:lang(), 'highlights')
    if not query then
      return
    end
    for id, node, metadata in query:iter_captures(tree:root(), source, start, stop) do
      local name = query.captures[id]
      if not name:match('^_') and name ~= 'spell' and name ~= 'nospell' and name ~= 'conceal' then
        local priority = tonumber(metadata.priority or (metadata[id] and metadata[id].priority)) or 100
        local fg = capture_fg(name, ltree:lang())
        if fg then
          local srow, scol, erow, ecol = node:range()
          table.insert(captures, { priority, #captures, srow, scol, erow, ecol, fg })
        end
      end
    end
  end)
  table.sort(captures, function(a, b)
    return a[1] < b[1] or (a[1] == b[1] and a[2] < b[2])
  end)
  return captures
end

-- Each of `lines`' color per byte, from `captures`: the last capture's to
-- cover it, or nil for none. `lines` begin at row `first`.
local function fill(captures, lines, first)
  local colors = {}
  for row = 1, #lines do
    colors[row] = {}
  end
  for _, c in ipairs(captures) do
    local srow, scol, erow, ecol, fg = c[3], c[4], c[5], c[6], c[7]
    for row = math.max(srow, first), math.min(erow, first + #lines - 1) do
      local line = lines[row - first + 1]
      local from = row == srow and scol or 0
      local to = row == erow and ecol or #line
      for col = from, to - 1 do
        colors[row - first + 1][col] = fg
      end
    end
  end
  return colors
end

-- Each code line's color per byte: its capture's, or nil for none.
local function highlight_code(code, lang)
  local ok, parser = pcall(vim.treesitter.get_string_parser, code, lang)
  if not ok then
    return nil
  end
  parser:parse(true)
  local lines = vim.split(code, '\n', { plain = true })
  return fill(captures_of(parser, code), lines, 0), lines
end

-- The foreground drawn for text with no color of its own.
local function normal_fg()
  return rgb(vim.api.nvim_get_hl(0, { name = 'Normal', link = false }).fg or 0xFFFFFF)
end

-- Color `text`, at `row` from byte `offset`, in `colors` tinted for `kind`:
-- runs of one color, each one extmark.
local function paint_row(buf, namespace, row, offset, text, colors, kind, default)
  local start, current = 0, nil
  for col = 0, #text do
    local name
    if col < #text then
      local fg = colors[col]
      name = group(tint(fg and rgb(fg) or default, kind))
    end
    if name ~= current then
      if current then
        vim.api.nvim_buf_set_extmark(buf, namespace, row, start + offset, {
          end_col = col + offset,
          hl_group = current,
        })
      end
      start, current = col, name
    end
  end
end

-- styra-diff's `is_file_marker`.
local function is_file_marker(line)
  return line:match('^%-%-%- a/') or line:match('^%-%-%- /dev/null')
    or line:match('^%+%+%+ b/') or line:match('^%+%+%+ /dev/null')
end

-- styra-diff's `is_code`.
local function is_code(line)
  return line == '' or line:match('^[-+ ]') ~= nil
end

-- Color a run of code rows, each `{ row, line }`, in `lang`.
local function paint(buf, hunk, lang)
  if #hunk == 0 or not lang then
    return
  end
  local code = {}
  for i, entry in ipairs(hunk) do
    code[i] = entry[2]:sub(2)
  end
  local colors, lines = highlight_code(table.concat(code, '\n'), lang)
  -- The highlighter must give one line per line of code.
  if not colors or #lines ~= #hunk then
    return
  end
  local default = normal_fg()
  for i, entry in ipairs(hunk) do
    local row, line = entry[1], entry[2]
    -- The code starts after the sign.
    paint_row(buf, ns, row, 1, lines[i], colors[i], line:sub(1, 1), default)
  end
end

-- Highlight and tint every hunk in `buf`, the current buffer by default.
function M.highlight(buf)
  buf = (buf == nil or buf == 0) and vim.api.nvim_get_current_buf() or buf
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  local lang, in_hunk, hunk = nil, false, {}
  local function flush()
    paint(buf, hunk, lang)
    hunk = {}
  end
  for row, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    row = row - 1
    local rest = line:match('^diff %-%-git (.*)$')
    if rest then
      flush()
      in_hunk = false
      lang = language_of(rest:match('^.* b/(.*)$') or rest)
    elseif line:match('^@@') then
      flush()
      in_hunk = true
    elseif not in_hunk or is_file_marker(line) then
      -- As in styra-diff, this replaces the language even when it names
      -- none, which a deleted file's `+++ /dev/null` does.
      local named = line:match('^%+%+%+ (.*)$')
      if named then
        lang = language_of((named:gsub('^b/', '')))
      end
      if is_code(line) and not is_file_marker(line) then
        table.insert(hunk, { row, line })
      else
        flush()
      end
    else
      -- `\ No newline at end of file` too, as styra-diff keeps it.
      table.insert(hunk, { row, line })
    end
  end
  flush()
end

-- Side by side: windows in diff mode, each showing one side of a change.
-- The oldest side is drawn as styra-diff draws removed rows and the others
-- as it draws added ones, with unchanged rows faded as its context is.

-- Each diff window's namespace and the buffer it was painted in. A
-- namespace is scoped to its window, so the same buffer in an ordinary
-- window keeps its plain colors.
local windows = {}

local function window_state(win)
  local state = windows[win]
  if not state then
    state = { ns = vim.api.nvim_create_namespace('styra_diff_window_' .. win) }
    pcall(vim.api.nvim__ns_set, state.ns, { wins = { win } })
    windows[win] = state
  end
  return state
end

local function forget(win)
  local state = windows[win]
  if state and state.buf and vim.api.nvim_buf_is_valid(state.buf) then
    vim.api.nvim_buf_clear_namespace(state.buf, state.ns, 0, -1)
  end
  windows[win] = nil
end

-- Each of `rows`' color per byte, as `win` draws it: from the buffer's
-- tree-sitter highlighter if it has one, else from its syntax.
local function drawn_colors(win, buf, rows)
  local first, last = rows[1], rows[#rows]
  local lines = vim.api.nvim_buf_get_lines(buf, first, last + 1, false)
  local highlighter = vim.treesitter.highlighter.active[buf]
  if highlighter then
    highlighter.tree:parse({ first, last + 1 })
    return fill(captures_of(highlighter.tree, buf, first, last + 1), lines, first), lines
  end
  local colors = {}
  for i = 1, #lines do
    colors[i] = {}
  end
  if vim.bo[buf].syntax ~= '' then
    local fgs = {}
    vim.api.nvim_win_call(win, function()
      for _, row in ipairs(rows) do
        local i = row - first + 1
        for col = 0, #lines[i] - 1 do
          local id = vim.fn.synIDtrans(vim.fn.synID(row + 1, col + 1, 1))
          if fgs[id] == nil then
            fgs[id] = id ~= 0 and vim.api.nvim_get_hl(0, { id = id, link = false }).fg or false
          end
          colors[i][col] = fgs[id] or nil
        end
      end
    end)
  end
  return colors, lines
end

local CHANGED = { DiffAdd = true, DiffChange = true, DiffText = true }

-- Band and tint the rows `win` shows, as the oldest side or another.
local function paint_window(win, oldest)
  local buf = vim.api.nvim_win_get_buf(win)
  local state = window_state(win)
  if state.buf and state.buf ~= buf and vim.api.nvim_buf_is_valid(state.buf) then
    vim.api.nvim_buf_clear_namespace(state.buf, state.ns, 0, -1)
  end
  state.buf = buf
  vim.api.nvim_buf_clear_namespace(buf, state.ns, 0, -1)
  local side = vim.w[win].styra_diff_side
  local changed = (side == 'old' or (side == nil and oldest)) and '-' or '+'
  -- Only the rows on screen; scrolling paints the rest.
  local rows, kinds = {}, {}
  vim.api.nvim_win_call(win, function()
    local lnum, bottom = vim.fn.line('w0'), vim.fn.line('w$')
    while lnum <= bottom do
      local fold_end = vim.fn.foldclosedend(lnum)
      if fold_end ~= -1 then
        lnum = fold_end + 1
      else
        local id = vim.fn.diff_hlID(lnum, 1)
        table.insert(rows, lnum - 1)
        kinds[lnum - 1] = CHANGED[vim.fn.synIDattr(id, 'name')] and changed or ' '
        lnum = lnum + 1
      end
    end
  end)
  if #rows == 0 then
    return
  end
  local colors, lines = drawn_colors(win, buf, rows)
  local default = normal_fg()
  for _, row in ipairs(rows) do
    local i, kind = row - rows[1] + 1, kinds[row]
    -- Diff mode's own line highlight would cover a `line_hl_group`; a
    -- highlight over the whole row, end of line included, is drawn over it.
    if kind ~= ' ' then
      vim.api.nvim_buf_set_extmark(buf, state.ns, row, 0, {
        end_row = row + 1,
        end_col = 0,
        hl_eol = true,
        hl_group = kind == '+' and 'styraDiffAddedLine' or 'styraDiffRemovedLine',
        priority = 1,
      })
    end
    -- Added rows keep the colors they have.
    if kind ~= '+' then
      paint_row(buf, state.ns, row, 0, lines[i], colors[i], kind, default)
    end
  end
end

-- Paint every diff window in the current tab page. The oldest side is the
-- leftmost window, or the topmost of those, unless a window's
-- `w:styra_diff_side` says 'old' or 'new'.
function M.side_by_side()
  local wins = {}
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.wo[win].diff and vim.api.nvim_win_get_config(win).relative == '' then
      table.insert(wins, win)
    end
  end
  table.sort(wins, function(a, b)
    local pa, pb = vim.api.nvim_win_get_position(a), vim.api.nvim_win_get_position(b)
    return pa[2] < pb[2] or (pa[2] == pb[2] and pa[1] < pb[1])
  end)
  -- A lone diff window has nothing to differ from.
  if #wins < 2 then
    wins = {}
  end
  local painted = {}
  for i, win in ipairs(wins) do
    paint_window(win, i == 1)
    painted[win] = true
  end
  for win in pairs(windows) do
    if not painted[win] and not (vim.api.nvim_win_is_valid(win)
        and vim.api.nvim_win_get_tabpage(win) ~= vim.api.nvim_get_current_tabpage()
        and vim.wo[win].diff) then
      forget(win)
    end
  end
end

-- Take every tint away, as another colorscheme takes over.
function M.clear_all()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  end
  for win in pairs(windows) do
    forget(win)
  end
  groups = {}
end

return M
