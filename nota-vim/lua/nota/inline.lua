-- Show the selected review's suggestions inside the files they change.
--
-- Each suggestion hunk is placed in every loaded buffer of its file. Hunk
-- lines refer to the suggestion's parent commit, so they are mapped onto the
-- buffer through a diff against that version and then checked: a hunk whose
-- original lines are still there is pending, one whose suggested lines are
-- already there (as in a worktree of the review branch) is applied, and
-- anything else is stale.

local M = {}

local ns = vim.api.nvim_create_namespace('nota_inline')
local diff = (vim.text and vim.text.diff) or vim.diff
local enabled = false
-- Per repository root: the review last loaded and its tip.
local reviews = {}
-- Immutable per commit and path: suggestion hunks and parent file contents.
local hunk_cache, base_cache = {}, {}
-- Per buffer: its repository file, rendered items, and expanded hunks.
local files, buffers = {}, {}

local function fail(message)
  error('nota: ' .. message, 0)
end

local function run(arguments)
  local result = vim.system(arguments, { text = true }):wait()
  if result.code ~= 0 then
    fail(vim.trim(result.stderr ~= '' and result.stderr or result.stdout))
  end
  return result.stdout
end

local function git(root, arguments)
  return run(vim.list_extend({ 'git', '-C', root, '--literal-pathspecs' }, arguments))
end

local function lines(text)
  local result = vim.split(text, '\n', { plain = true })
  if result[#result] == '' then
    table.remove(result)
  end
  return result
end

local function highlights()
  for name, link in pairs({
    NotaInlineSign = 'DiagnosticInfo',
    NotaInlineApplied = 'DiagnosticOk',
    NotaInlineStale = 'Comment',
    NotaInlineSummary = 'DiagnosticVirtualTextInfo',
    NotaInlineAdd = 'DiffAdd',
    NotaInlineDelete = 'DiffDelete',
  }) do
    vim.api.nvim_set_hl(0, name, { link = link, default = true })
  end
end

-- The repository root and worktree-relative path of a file buffer, or nil.
local function file(buf)
  local name = vim.api.nvim_buf_get_name(buf)
  local cached = files[buf]
  if cached and cached.name == name then
    return cached.root and cached
  end
  local info = { name = name }
  files[buf] = info
  if name == '' or vim.bo[buf].buftype ~= '' then
    return nil
  end
  local ok, root = pcall(git, vim.fn.fnamemodify(name, ':p:h'), { 'rev-parse', '--show-toplevel' })
  if not ok then
    return nil
  end
  root = vim.trim(root)
  local real_root = vim.uv.fs_realpath(root) or root
  local real_name = vim.uv.fs_realpath(name) or vim.fn.fnamemodify(name, ':p')
  local prefix = real_root .. '/'
  if real_name:sub(1, #prefix) ~= prefix then
    return nil
  end
  info.root, info.path = root, real_name:sub(#prefix + 1)
  return info
end

-- The review for this worktree, reloaded when its branch or tip moves. Like
-- the other commands, use the selection or else the checked-out branch.
local function review(root)
  local branch = vim.fn['nota#selected'](root)
  local tip
  if branch == '' then
    local head = lines(git(root, { 'rev-parse', 'HEAD', '--symbolic-full-name', 'HEAD' }))
    branch = (head[2] or ''):match('^refs/heads/(.+)$')
    if not branch then
      return { entries = {}, error = 'no review selected and no branch checked out' }
    end
    tip = head[1]
  else
    tip = vim.trim(git(root, { 'rev-parse', '--verify', 'refs/heads/' .. branch .. '^{commit}' }))
  end
  local cached = reviews[root]
  if cached and cached.branch == branch and cached.tip == tip then
    return cached
  end
  local executable = vim.g.nota_executable or 'nota'
  local ok, output = pcall(run, {
    executable, 'show', '--json', '--repository=' .. root, '--branch=' .. branch,
  })
  local loaded = { branch = branch, tip = tip, entries = {} }
  if ok then
    loaded.entries = vim.json.decode(output).entries
  else
    loaded.error = output
  end
  reviews[root] = loaded
  return loaded
end

local function hunks(root, commit, path)
  local key = commit .. '\0' .. path
  if hunk_cache[key] then
    return hunk_cache[key]
  end
  local output = git(root, {
    'diff', '-U0', '--no-color', '--no-ext-diff', '--no-renames', '--no-relative',
    commit .. '^', commit, '--', path,
  })
  local result, hunk = {}, nil
  for _, line in ipairs(lines(output)) do
    local old_start, old_count, new_start, new_count =
      line:match('^@@ %-(%d+),?(%d*) %+(%d+),?(%d*) @@')
    if old_start then
      hunk = {
        index = #result + 1,
        old_start = tonumber(old_start),
        old_count = old_count == '' and 1 or tonumber(old_count),
        new_start = tonumber(new_start),
        new_count = new_count == '' and 1 or tonumber(new_count),
        old = {},
        new = {},
      }
      table.insert(result, hunk)
    elseif hunk and line:sub(1, 1) == '-' then
      table.insert(hunk.old, line:sub(2))
    elseif hunk and line:sub(1, 1) == '+' then
      table.insert(hunk.new, line:sub(2))
    end
  end
  hunk_cache[key] = result
  return result
end

-- The file as the suggestion found it; empty when the suggestion adds it.
local function base(root, commit, path)
  local key = commit .. '\0' .. path
  if not base_cache[key] then
    local ok, output = pcall(git, root, { 'cat-file', 'blob', commit .. '^:' .. path })
    base_cache[key] = ok and lines(output) or {}
  end
  return base_cache[key]
end

local function matches(buffer, first, expected)
  if #expected == 0 or first < 1 or first + #expected - 1 > #buffer then
    return false
  end
  for offset, line in ipairs(expected) do
    if buffer[first + offset - 1] ~= line then
      return false
    end
  end
  return true
end

-- Where each hunk lands in the buffer, and whether it is pending, applied,
-- or stale there.
local function place(changes, old, buffer)
  local text = function(list)
    return #list == 0 and '' or table.concat(list, '\n') .. '\n'
  end
  local moves = diff(text(old), text(buffer), { result_type = 'indices' })
  -- Map a parent line through the buffer changes that end before it.
  local function map(line)
    local result = line
    for _, move in ipairs(moves) do
      local start_a, count_a, _, count_b = unpack(move)
      -- An insertion's start is the line it follows.
      local last = count_a == 0 and start_a or start_a + count_a - 1
      if last < line then
        result = result + count_b - count_a
      end
    end
    return result
  end
  local function deleted(change)
    for _, move in ipairs(moves) do
      if move[1] == change.old_start and move[2] == change.old_count and move[4] == 0 then
        return true
      end
    end
    return false
  end
  local placed = {}
  for _, change in ipairs(changes) do
    local item = { change = change }
    if change.old_count == 0 then
      -- An insertion after a parent line, or at the top for line 0.
      local anchor = change.old_start == 0 and 0 or map(change.old_start)
      item.line = anchor
      if matches(buffer, anchor + 1, change.new) then
        item.status, item.line = 'applied', anchor + 1
      elseif change.old_start == 0 or buffer[anchor] == old[change.old_start] then
        item.status = 'pending'
      else
        item.status = 'stale'
      end
    else
      item.line = map(change.old_start)
      if matches(buffer, item.line, change.old) then
        item.status = 'pending'
      elseif change.new_count > 0 and matches(buffer, item.line, change.new)
          or change.new_count == 0 and deleted(change) then
        item.status = 'applied'
      else
        item.status = 'stale'
      end
    end
    table.insert(placed, item)
  end
  return placed
end

-- Virtual text shows tabs literally; expand them one at a time, since each
-- expansion moves the columns of the tabs after it.
local function expand_tabs(line, tabstop)
  local position = line:find('\t', 1, true)
  while position do
    local width = tabstop - (vim.fn.strdisplaywidth(line:sub(1, position - 1)) % tabstop)
    line = line:sub(1, position - 1) .. string.rep(' ', width) .. line:sub(position + 1)
    position = line:find('\t', position, true)
  end
  return line
end

local function virtual(buf, list, group, prefix)
  local tabstop = vim.bo[buf].tabstop
  local result = {}
  for _, line in ipairs(list) do
    local shown = (prefix or '') .. expand_tabs(line, tabstop)
    local padding = math.max(vim.o.columns - vim.fn.strdisplaywidth(shown), 0)
    table.insert(result, { { shown .. string.rep(' ', padding), group } })
  end
  return result
end

local function summary(entry)
  return entry.message:match('^[^\n]*')
end

local function mark(buf, row, options)
  return vim.api.nvim_buf_set_extmark(buf, ns, row, 0, options)
end

-- Draw one placed hunk. Returns the extmark that anchors its first row.
local function draw(buf, entry, item, expanded, last_row)
  local change = item.change
  local status = item.status
  local sign_group = status == 'pending' and 'NotaInlineSign'
    or status == 'applied' and 'NotaInlineApplied' or 'NotaInlineStale'
  local sign_text = status == 'stale' and '?' or '▎'
  -- The buffer rows this hunk covers: original lines while pending,
  -- suggested lines once applied.
  local count = status == 'pending' and change.old_count
    or status == 'applied' and change.new_count or 0
  local first = math.min(math.max(item.line - 1, 0), last_row)
  if count == 0 then
    count = 1
  end
  local rows = math.min(count, last_row - first + 1)

  local label = string.format('✎ %s %s', entry.commit:sub(1, 8), summary(entry))
  if status ~= 'pending' then
    label = label .. ' (' .. status .. ')'
  end
  local anchor = mark(buf, first, {
    sign_text = sign_text,
    sign_hl_group = sign_group,
    virt_text = { { label, 'NotaInlineSummary' } },
    virt_text_pos = 'eol',
  })
  for row = first + 1, first + rows - 1 do
    mark(buf, row, { sign_text = sign_text, sign_hl_group = sign_group })
  end
  if not expanded then
    return anchor, first, rows
  end

  if status == 'pending' then
    if change.old_count > 0 then
      for row = first, first + rows - 1 do
        mark(buf, row, { line_hl_group = 'NotaInlineDelete' })
      end
    end
    if change.new_count > 0 then
      local insert_at_top = change.old_count == 0 and item.line == 0
      mark(buf, insert_at_top and 0 or first + rows - 1, {
        virt_lines = virtual(buf, change.new, 'NotaInlineAdd'),
        virt_lines_above = insert_at_top,
      })
    end
  elseif status == 'applied' then
    if change.new_count > 0 then
      for row = first, first + rows - 1 do
        mark(buf, row, { line_hl_group = 'NotaInlineAdd' })
      end
    end
    if change.old_count > 0 then
      -- Removed lines sit where they were, which may be past the last row.
      local past_end = item.line - 1 > last_row
      mark(buf, first, {
        virt_lines = virtual(buf, change.old, 'NotaInlineDelete'),
        virt_lines_above = not past_end,
      })
    end
  else
    local lines_shown = virtual(buf, change.old, 'NotaInlineDelete', '- ')
    vim.list_extend(lines_shown, virtual(buf, change.new, 'NotaInlineAdd', '+ '))
    mark(buf, first, { virt_lines = lines_shown })
  end
  return anchor, first, rows
end

local function set_mappings(buf, state)
  if state.mapped or vim.g.nota_inline_mappings == 0 then
    return
  end
  local function map(lhs, rhs, desc)
    vim.keymap.set('n', lhs, rhs, { buffer = buf, silent = true, desc = desc })
  end
  map(']r', function() M.jump(vim.v.count1) end, 'Next Nota review item')
  map('[r', function() M.jump(-vim.v.count1) end, 'Previous Nota review item')
  map('<leader>re', function() M.toggle_item() end, 'Expand or collapse Nota review item')
  map('<leader>rE', function() M.toggle_all() end, 'Expand or collapse all Nota review items')
  state.mapped = true
end

local function clear_mappings(buf, state)
  if not state.mapped then
    return
  end
  for _, lhs in ipairs({ ']r', '[r', '<leader>re', '<leader>rE' }) do
    pcall(vim.keymap.del, 'n', lhs, { buffer = buf })
  end
  state.mapped = false
end

local function clear(buf)
  if vim.api.nvim_buf_is_valid(buf) then
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  end
  local state = buffers[buf]
  if state then
    state.items = {}
    if vim.api.nvim_buf_is_valid(buf) then
      clear_mappings(buf, state)
    end
  end
end

-- Draw the review into one buffer. `loaded` caches reviews within one pass;
-- `cached` reuses the last review without asking Git whether it moved.
local function render(buf, loaded, cached)
  if not vim.api.nvim_buf_is_loaded(buf) then
    return
  end
  local info = file(buf)
  if not info then
    return clear(buf)
  end
  local current
  if cached then
    current = reviews[info.root]
  elseif loaded and loaded[info.root] then
    current = loaded[info.root]
  else
    local ok, result = pcall(review, info.root)
    current = ok and result or { entries = {}, error = result }
    if loaded then
      loaded[info.root] = current
    end
  end
  local state = buffers[buf] or { expanded = {}, items = {} }
  buffers[buf] = state
  clear(buf)
  if not current then
    return
  end

  local buffer = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local last_row = math.max(#buffer - 1, 0)
  local default = vim.g.nota_inline_expanded == 1
  for _, entry in ipairs(current.entries) do
    if entry.kind == 'suggestion' and vim.tbl_contains(entry.paths, info.path) then
      local ok, changes = pcall(hunks, info.root, entry.commit, info.path)
      if ok then
        local old = base(info.root, entry.commit, info.path)
        for _, item in ipairs(place(changes, old, buffer)) do
          local key = entry.commit .. ':' .. item.change.index
          local expanded = state.expanded[key]
          if expanded == nil then
            expanded = default
          end
          local anchor, _, rows = draw(buf, entry, item, expanded, last_row)
          table.insert(state.items, { id = anchor, rows = rows, key = key, expanded = expanded })
        end
      end
    end
  end
  if #state.items > 0 then
    set_mappings(buf, state)
  end
end

local function file_buffers()
  return vim.tbl_filter(function(buf)
    return vim.api.nvim_buf_is_loaded(buf) and vim.bo[buf].buftype == ''
      and vim.api.nvim_buf_get_name(buf) ~= ''
  end, vim.api.nvim_list_bufs())
end

-- The items in the current buffer, ordered by their current rows.
local function items()
  local buf = vim.api.nvim_get_current_buf()
  local state = buffers[buf]
  if not state or #state.items == 0 then
    fail('no review items in this buffer')
  end
  local result = {}
  for _, item in ipairs(state.items) do
    local position = vim.api.nvim_buf_get_extmark_by_id(buf, ns, item.id, {})
    if position[1] then
      table.insert(result, vim.tbl_extend('force', item, { row = position[1] }))
    end
  end
  table.sort(result, function(a, b) return a.row < b.row end)
  return result, state, buf
end

local function report(fn)
  local ok, message = pcall(fn)
  if not ok then
    local text = tostring(message):gsub('^nota:', 'Nota:')
    vim.api.nvim_echo({ { text, 'ErrorMsg' } }, true, {})
  end
end

function M.jump(count)
  report(function()
    local list = items()
    local row = vim.api.nvim_win_get_cursor(0)[1] - 1
    local rows = {}
    for _, item in ipairs(list) do
      if rows[#rows] ~= item.row then
        table.insert(rows, item.row)
      end
    end
    local target
    if count > 0 then
      local seen = 0
      for _, candidate in ipairs(rows) do
        if candidate > row then
          seen = seen + 1
          target = candidate
          if seen == count then
            break
          end
        end
      end
    else
      local seen = 0
      for index = #rows, 1, -1 do
        if rows[index] < row then
          seen = seen + 1
          target = rows[index]
          if seen == -count then
            break
          end
        end
      end
    end
    if not target then
      fail(count > 0 and 'no later review item' or 'no earlier review item')
    end
    vim.cmd("normal! m'")
    vim.api.nvim_win_set_cursor(0, { target + 1, 0 })
  end)
end

function M.toggle_item()
  report(function()
    local list, state, buf = items()
    local row = vim.api.nvim_win_get_cursor(0)[1] - 1
    local found = {}
    for _, item in ipairs(list) do
      if item.row <= row and row < item.row + item.rows then
        table.insert(found, item)
      end
    end
    if #found == 0 then
      fail('no review item under the cursor')
    end
    -- Overlapping hunks from stacked suggestions toggle together.
    local expand = not found[1].expanded
    for _, item in ipairs(found) do
      state.expanded[item.key] = expand
    end
    render(buf, nil, true)
  end)
end

function M.toggle_all()
  report(function()
    local list, state, buf = items()
    local expand = false
    for _, item in ipairs(list) do
      if not item.expanded then
        expand = true
      end
    end
    for _, item in ipairs(list) do
      state.expanded[item.key] = expand
    end
    render(buf, nil, true)
  end)
end

-- Redraw every loaded buffer of `root`, or of every repository.
function M.refresh(root)
  if not enabled then
    return
  end
  local loaded = {}
  for _, buf in ipairs(file_buffers()) do
    local info = file(buf)
    if not root or info and info.root == root then
      render(buf, loaded, false)
    end
  end
end

local function enable()
  enabled = true
  highlights()
  local group = vim.api.nvim_create_augroup('nota_inline', { clear = true })
  vim.api.nvim_create_autocmd({ 'BufReadPost', 'BufWinEnter', 'BufWritePost' }, {
    group = group,
    callback = function(event) render(event.buf, nil, false) end,
  })
  vim.api.nvim_create_autocmd({ 'TextChanged', 'InsertLeave' }, {
    group = group,
    callback = function(event)
      if buffers[event.buf] then
        render(event.buf, nil, true)
      end
    end,
  })
  vim.api.nvim_create_autocmd('BufWipeout', {
    group = group,
    callback = function(event)
      files[event.buf], buffers[event.buf] = nil, nil
    end,
  })
  vim.api.nvim_create_autocmd('ColorScheme', { group = group, callback = highlights })
  M.refresh()
  local info = file(vim.api.nvim_get_current_buf())
  local current = info and reviews[info.root]
  if current and current.error then
    vim.api.nvim_echo({ { 'Nota: ' .. current.error, 'WarningMsg' } }, true, {})
  elseif current then
    vim.api.nvim_echo({ { 'Nota: showing ' .. current.branch .. ' inline' } }, true, {})
  end
end

local function disable()
  enabled = false
  pcall(vim.api.nvim_del_augroup_by_name, 'nota_inline')
  for buf in pairs(buffers) do
    clear(buf)
  end
  buffers = {}
  vim.api.nvim_echo({ { 'Nota: inline display off' } }, true, {})
end

function M.toggle()
  if enabled then
    disable()
  else
    enable()
  end
end

function M.enabled()
  return enabled
end

M.namespace = ns

return M
