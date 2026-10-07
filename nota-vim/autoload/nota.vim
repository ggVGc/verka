let s:branches = {}
let s:buffer_id = 0

" Use one error boundary for commands, buffer mappings, and :write callbacks.
function! nota#command(action, arguments) abort
  try
    return call(function('s:' . a:action), a:arguments)
  catch /^nota:/
    echohl ErrorMsg
    echomsg substitute(v:exception, '^nota:', 'Nota:', '')
    echohl None
    return 0
  endtry
endfunction

" The review selected for a worktree root, or '' for the checked-out branch.
function! nota#selected(repository) abort
  return get(s:branches, a:repository, '')
endfunction

" Redraw inline review items after the selection or the review changes.
function! s:inline(repository) abort
  if has('nvim')
    call luaeval("require('nota.inline').refresh(_A)", a:repository)
  endif
endfunction

function! s:fail(message) abort
  throw 'nota: ' . a:message
endfunction

function! s:run(arguments) abort
  if !executable(a:arguments[0])
    call s:fail('executable not found: ' . a:arguments[0])
  endif
  " Escape each argument separately, including multiline prose and paths.
  let l:command = join(map(copy(a:arguments), 'shellescape(v:val)'), ' ')
  let l:lines = systemlist(l:command . ' 2>&1')
  if v:shell_error
    call s:fail(join(l:lines, "\n"))
  endif
  return l:lines
endfunction

function! s:git(repository, arguments) abort
  return s:run(['git', '-C', a:repository] + a:arguments)
endfunction

function! s:context() abort
  if exists('b:nota_context')
    return copy(b:nota_context)
  endif
  let l:path = expand('%:p')
  let l:directory = empty(l:path) || &buftype !=# '' ? getcwd() : fnamemodify(l:path, ':h')
  let l:root = s:git(l:directory, ['rev-parse', '--show-toplevel'])[0]
  return {'repository': l:root, 'branch': get(s:branches, l:root, '')}
endfunction

function! s:cli(context, command, arguments, ...) abort
  let l:args = [get(g:, 'nota_executable', 'nota'), a:command,
        \ '--repository=' . a:context.repository] + get(a:, 1, [])
  if !empty(a:context.branch)
    call add(l:args, '--branch=' . a:context.branch)
  endif
  " Positional arguments after -- cannot become CLI options.
  return s:run(l:args + ['--'] + a:arguments)
endfunction

" Reviews started from the checked-out branch, as named by `nota start`:
" nota/review-<branch>, or a numbered sibling that is not the review of
" another branch.
function! s:associated(repository, current, reviews) abort
  let l:base = 'nota/review-' . a:current
  let l:branches = s:git(a:repository, ['for-each-ref', '--format=%(refname:short)', 'refs/heads/'])
  return filter(copy(a:reviews), {_, review -> review ==# l:base
        \ || stridx(review, l:base . '-') == 0 && review[len(l:base) + 1 :] =~# '^\d\+$'
        \ && index(l:branches, a:current . review[len(l:base) :]) < 0})
endfunction

" With no selection and no review checked out, select the review of the
" checked-out branch, asking which when there are several.
function! s:associate(context) abort
  if !empty(a:context.branch)
    return
  endif
  let l:current = get(s:git(a:context.repository, ['branch', '--show-current']), 0, '')
  if empty(l:current)
    return
  endif
  let l:index = json_decode(join(s:run([get(g:, 'nota_executable', 'nota'), 'list',
        \ '--repository=' . a:context.repository, '--json']), "\n"))
  let l:reviews = map(copy(l:index.reviews), 'v:val.branch')
  if index(l:reviews, l:current) >= 0
    return
  endif
  let l:candidates = s:associated(a:context.repository, l:current, l:reviews)
  if empty(l:candidates)
    return
  endif
  let l:choice = 1
  if len(l:candidates) > 1
    let l:choice = inputlist(['Nota: select a review of ' . l:current . ':']
          \ + map(copy(l:candidates), {i, branch -> (i + 1) . '. ' . branch}))
    redraw
    if l:choice < 1 || l:choice > len(l:candidates)
      call s:fail('no review selected')
    endif
  endif
  let a:context.branch = l:candidates[l:choice - 1]
  let s:branches[a:context.repository] = a:context.branch
  call s:inline(a:context.repository)
  echomsg 'Nota: selected ' . a:context.branch . ', the review of ' . l:current
endfunction

function! s:review_branch(lines) abort
  for l:line in a:lines
    let l:branch = matchstr(l:line, '^review\s\+\zs\S\+\ze\s*$')
    if !empty(l:branch)
      return l:branch
    endif
  endfor
  call s:fail('nota did not report a review branch')
endfunction

" Load and validate the review, resolving the context's default branch.
function! s:load(context) abort
  let l:review = json_decode(join(s:cli(a:context, 'show', [], ['--json']), "\n"))
  let a:context.branch = l:review.branch
  return l:review
endfunction

function! s:summary(entry) abort
  return get(split(a:entry.message, "\n"), 0, '')
endfunction

" The review as text, and the commit each line opens with <CR>.
function! s:render(review) abort
  let l:lines = ['review   ' . a:review.branch, 'subject  ' . a:review.subject,
        \ 'marker   ' . a:review.marker]
  let l:commits = ['', a:review.subject, '']
  if empty(a:review.entries)
    call add(l:lines, 'entries  none')
    call add(l:commits, '')
  endif
  for l:entry in a:review.entries
    call add(l:lines, printf('%s  %-10s %s', strpart(l:entry.commit, 0, 12),
          \ l:entry.kind, s:summary(l:entry)))
    call add(l:commits, l:entry.commit)
    for l:path in l:entry.paths
      call add(l:lines, repeat(' ', 13) . l:path)
      call add(l:commits, l:entry.commit)
    endfor
  endfor
  return [l:lines, l:commits]
endfunction

function! s:scratch(kind, context) abort
  botright new
  let s:buffer_id += 1
  execute 'file nota://' . a:kind . '/' . s:buffer_id
  setlocal buftype=nofile bufhidden=wipe noswapfile nobuflisted
  let b:nota_context = copy(a:context)
  nnoremap <silent><buffer> q :close<CR>
endfunction

function! s:display(context, review) abort
  call s:scratch('review', a:context)
  call s:fill(a:review)
  setlocal filetype=nota
  nnoremap <silent><buffer> r :call nota#command('refresh', [])<CR>
  nnoremap <silent><buffer> <CR> :call nota#command('entry', [])<CR>
  nnoremap <silent><buffer> gq :NotaQuickfix<CR>
  nnoremap <silent><buffer> n :NotaNote<CR>
  nnoremap <silent><buffer> s :NotaSuggest<CR>
  nnoremap <silent><buffer> p :call nota#command('toggle_preview', [])<CR>
  let b:nota_preview = get(g:, 'nota_preview', 1)
  let b:nota_preview_window = 0
  let b:nota_previewed = ''
  let b:nota_patches = {}
  augroup nota_preview
    autocmd! * <buffer>
    autocmd CursorMoved <buffer> call nota#command('preview', [])
    autocmd BufWinLeave <buffer> call s:close_preview(str2nr(expand('<abuf>')))
  augroup END
  return s:preview()
endfunction

" Show the entry under the cursor in a split beside the review buffer. Lines
" without an entry keep the last one shown.
function! s:preview() abort
  if !b:nota_preview
    return 1
  endif
  if b:nota_preview_window && !win_id2win(b:nota_preview_window)
    " Closed by hand: stay closed until toggled on again.
    let b:nota_preview = 0
    let b:nota_preview_window = 0
    return 1
  endif
  let l:commit = get(b:nota_commits, line('.') - 1, '')
  if empty(l:commit) || l:commit ==# b:nota_previewed
    return 1
  endif
  " Commits never change, so each is read from Git once.
  if !has_key(b:nota_patches, l:commit)
    let b:nota_patches[l:commit] = s:patch(b:nota_context, l:commit)
  endif
  let l:lines = b:nota_patches[l:commit]
  if !b:nota_preview_window
    let l:review = win_getid()
    rightbelow vertical new
    let s:buffer_id += 1
    execute 'file nota://preview/' . s:buffer_id
    setlocal buftype=nofile bufhidden=wipe noswapfile nobuflisted filetype=git
    let l:window = win_getid()
    call win_gotoid(l:review)
    let b:nota_preview_window = l:window
  endif
  let l:buffer = winbufnr(b:nota_preview_window)
  call setbufvar(l:buffer, '&modifiable', 1)
  call deletebufline(l:buffer, 1, '$')
  call setbufline(l:buffer, 1, l:lines)
  call setbufvar(l:buffer, '&modifiable', 0)
  call setbufvar(l:buffer, '&modified', 0)
  call win_execute(b:nota_preview_window, 'call cursor(1, 1)')
  let b:nota_previewed = l:commit
  return 1
endfunction

function! s:close_preview(buffer) abort
  let l:window = win_id2win(getbufvar(a:buffer, 'nota_preview_window', 0))
  if l:window
    execute l:window . 'close'
  endif
  call setbufvar(a:buffer, 'nota_preview_window', 0)
  call setbufvar(a:buffer, 'nota_previewed', '')
endfunction

function! s:toggle_preview() abort
  if b:nota_preview && win_id2win(b:nota_preview_window)
    call s:close_preview(bufnr('%'))
    let b:nota_preview = 0
    return 1
  endif
  let b:nota_preview = 1
  let b:nota_preview_window = 0
  return s:preview()
endfunction

function! s:start(...) abort
  if a:0 > 2
    call s:fail('usage: NotaStart [revision] [branch]')
  endif
  let l:context = s:context()
  let l:context.branch = a:0 == 2 ? a:2 : ''
  let l:lines = s:cli(l:context, 'start', [a:0 ? a:1 : 'HEAD'])
  let l:context.branch = s:review_branch(l:lines)
  let s:branches[l:context.repository] = l:context.branch
  call s:inline(l:context.repository)
  for l:line in l:lines
    echomsg l:line
  endfor
  return s:display(l:context, s:load(l:context))
endfunction

function! s:show(...) abort
  let l:context = s:context()
  if a:0
    let l:context.branch = a:1
  endif
  call s:associate(l:context)
  let l:review = s:load(l:context)
  let s:branches[l:context.repository] = l:context.branch
  call s:inline(l:context.repository)
  return s:display(l:context, l:review)
endfunction

function! s:branch(...) abort
  let l:context = s:context()
  if !a:0
    if has_key(s:branches, l:context.repository)
      call remove(s:branches, l:context.repository)
    endif
    echomsg 'Nota: using the checked-out branch for ' . l:context.repository
  else
    let l:context.branch = a:1
    call s:load(l:context)
    let s:branches[l:context.repository] = l:context.branch
    echomsg 'Nota: selected ' . l:context.branch
  endif
  call s:inline(l:context.repository)
  return 1
endfunction

function! s:toggle() abort
  if !luaeval("require('nota.inline').enabled()")
    " Outside a repository there is nothing to select.
    try
      let l:context = s:context()
    catch /^nota:/
      let l:context = {}
    endtry
    if !empty(l:context)
      call s:associate(l:context)
    endif
  endif
  lua require('nota.inline').toggle()
  return 1
endfunction

" The lines a note is about, with the buffer text they number: Nota records
" them against the commit checked out, carrying them over unsaved and
" uncommitted edits.
function! s:location(context, first, last) abort
  if &buftype !=# '' || empty(expand('%:p'))
    call s:fail('a line range needs a file buffer')
  endif
  let l:path = expand('%:p')
  let l:prefix = a:context.repository . '/'
  if stridx(l:path, l:prefix) == 0
    let l:path = strpart(l:path, strlen(l:prefix))
  endif
  return {'path': l:path, 'first': a:first, 'last': a:last, 'contents': getline(1, '$')}
endfunction

function! s:add_note(context, text, source) abort
  if a:text !~# '\S'
    call s:fail('review message must not be empty')
  endif
  let l:options = []
  if !empty(a:source)
    let l:contents = tempname()
    call writefile(a:source.contents, l:contents)
    let l:options = ['--path=' . a:source.path,
          \ '--lines=' . a:source.first . '-' . a:source.last, '--contents=' . l:contents]
  endif
  try
    let l:lines = s:cli(a:context, 'note', [a:text], l:options)
  finally
    if exists('l:contents')
      call delete(l:contents)
    endif
  endtry
  call s:inline(a:context.repository)
  echomsg 'Nota: ' . join(l:lines, "\n")
  return 1
endfunction

" Run git with a scratch index, leaving the worktree's own index alone.
function! s:git_index(index, repository, arguments) abort
  let l:saved = exists('$GIT_INDEX_FILE') ? $GIT_INDEX_FILE : v:null
  let $GIT_INDEX_FILE = a:index
  try
    return s:git(a:repository, a:arguments)
  finally
    if l:saved is v:null
      unlet $GIT_INDEX_FILE
    else
      let $GIT_INDEX_FILE = l:saved
    endif
  endtry
endfunction

function! s:unsaved(repository) abort
  let l:prefix = a:repository . '/'
  for l:buffer in getbufinfo({'bufmodified': 1})
    if getbufvar(l:buffer.bufnr, '&buftype') ==# ''
          \ && stridx(fnamemodify(l:buffer.name, ':p'), l:prefix) == 0
      call s:fail('write ' . fnamemodify(l:buffer.name, ':~:.') . ' before suggesting')
    endif
  endfor
endfunction

" Reload unmodified buffers whose files changed on disk, without prompting.
function! s:reload(repository) abort
  let l:prefix = a:repository . '/'
  let l:autoread = &autoread
  set autoread
  try
    for l:buffer in getbufinfo({'bufloaded': 1})
      if getbufvar(l:buffer.bufnr, '&buftype') ==# ''
            \ && stridx(fnamemodify(l:buffer.name, ':p'), l:prefix) == 0
        execute 'silent! checktime ' . l:buffer.bufnr
      endif
    endfor
  finally
    let &autoread = l:autoread
  endtry
endfunction

" A text blob as its lines and whether the last one ends in a newline.
function! s:blob(repository, object) abort
  let l:text = system(join(map(['git', '-C', a:repository, 'cat-file', 'blob', a:object],
        \ 'shellescape(v:val)'), ' '))
  if v:shell_error
    call s:fail(l:text)
  endif
  let l:lines = split(l:text, "\n", 1)
  let l:newline = l:lines[-1] ==# ''
  return {'lines': l:newline ? l:lines[: -2] : l:lines, 'newline': l:newline}
endfunction

" The changes from one blob to another, as base line ranges and their
" replacements; v:null for a binary file. A change of as many lines as it
" replaces is split into single lines, so edits beside each one still merge;
" the last line's newline is left to the caller.
function! s:hunks(repository, base, other) abort
  let l:changes = []
  let l:lines = a:other.lines
  let l:diff = s:git(a:repository, ['diff', '-U0', '--no-color', '--no-ext-diff',
        \ '--no-textconv', a:base.object, a:other.object])
  for l:line in l:diff
    if l:line =~# '^Binary files '
      return v:null
    endif
    let l:match = matchlist(l:line, '^@@ -\(\d\+\)\%(,\(\d\+\)\)\? +\(\d\+\)\%(,\(\d\+\)\)\? @@')
    if empty(l:match)
      continue
    endif
    let l:count = l:match[2] ==# '' ? 1 : str2nr(l:match[2])
    let l:added = l:match[4] ==# '' ? 1 : str2nr(l:match[4])
    " A pure insertion follows base line a; a change starts at line a.
    let l:start = str2nr(l:match[1]) - (l:count ? 1 : 0)
    let l:from = str2nr(l:match[3]) - 1
    let l:replacement = l:added ? l:lines[l:from : l:from + l:added - 1] : []
    if l:count == l:added
      for l:index in range(l:count)
        " A last line that only gained or lost its newline is unchanged here.
        if l:replacement[l:index] !=# a:base.lines[l:start + l:index]
          call add(l:changes, {'start': l:start + l:index, 'count': 1,
                \ 'lines': [l:replacement[l:index]]})
        endif
      endfor
    else
      call add(l:changes, {'start': l:start, 'count': l:count, 'lines': l:replacement})
    endif
  endfor
  return l:changes
endfunction

" Merge two sets of changes to the base lines; v:null when they change the
" same lines or insert at the same place.
function! s:merge(base, ours, theirs) abort
  let l:changes = sort(a:ours + a:theirs,
        \ {a, b -> a.start != b.start ? a.start - b.start : a.count - b.count})
  let l:lines = []
  let l:next = 0
  let l:previous = {}
  for l:change in l:changes
    if l:change ==# l:previous
      continue
    endif
    if l:change.start < l:next || !l:change.count && !empty(l:previous)
          \ && !l:previous.count && l:previous.start == l:change.start
      return v:null
    endif
    if l:change.start > l:next
      call extend(l:lines, a:base[l:next : l:change.start - 1])
    endif
    call extend(l:lines, l:change.lines)
    let l:next = l:change.start + l:change.count
    let l:previous = l:change
  endfor
  return l:lines + a:base[l:next :]
endfunction

" Git's merge also refuses edits beside lines the other side changed. Merge
" each conflicted file in the index again, line by line, refusing only edits
" to the same lines. Returns whether every conflict was resolved.
function! s:resolve(index, repository) abort
  let l:paths = {}
  for l:entry in s:git_index(a:index, a:repository, ['ls-files', '--unmerged'])
    let l:match = matchlist(l:entry, '^\(\d\+\) \(\x\+\) \([123]\)\t\(.*\)$')
    " Quoted paths hold characters this cannot pass back to Git.
    if empty(l:match) || l:match[4] =~# '^"'
      return 0
    endif
    let l:stages = get(l:paths, l:match[4], {})
    let l:stages[l:match[3]] = {'mode': l:match[1], 'object': l:match[2]}
    let l:paths[l:match[4]] = l:stages
  endfor
  if empty(l:paths)
    return 0
  endif
  let l:file = tempname()
  try
    for [l:path, l:stages] in items(l:paths)
      if len(l:stages) != 3
        return 0
      endif
      let [l:base, l:ours, l:theirs] = [l:stages[1], l:stages[2], l:stages[3]]
      call extend(l:base, s:blob(a:repository, l:base.object))
      call extend(l:ours, s:blob(a:repository, l:ours.object))
      call extend(l:theirs, s:blob(a:repository, l:theirs.object))
      let l:mine = s:hunks(a:repository, l:base, l:ours)
      let l:yours = s:hunks(a:repository, l:base, l:theirs)
      if l:mine is v:null || l:yours is v:null
        return 0
      endif
      let l:lines = s:merge(l:base.lines, l:mine, l:yours)
      if l:lines is v:null
        return 0
      endif
      let l:newline = l:theirs.newline != l:base.newline ? l:theirs.newline : l:ours.newline
      let l:mode = l:theirs.mode !=# l:base.mode ? l:theirs.mode : l:ours.mode
      call writefile(l:lines + (l:newline ? [''] : []), l:file, 'b')
      let l:object = s:git(a:repository, ['hash-object', '-w', '--no-filters', '--', l:file])[0]
      call s:git_index(a:index, a:repository,
            \ ['update-index', '--cacheinfo', l:mode . ',' . l:object . ',' . l:path])
    endfor
  finally
    call delete(l:file)
  endtry
  return 1
endfunction

" Commit the worktree's uncommitted edits to the review branch, then undo
" them in the checkout and index: they now live in the review. The edits are
" merged onto the review as a patch against HEAD, so they keep the review's
" earlier changes; conflicting edits are refused.
function! s:add_suggestion(context, text) abort
  let l:text = substitute(a:text, '\_s*$', '', '')
  if l:text !~# '\S'
    call s:fail('review message must not be empty')
  endif
  let l:repository = a:context.repository
  let l:ref = 'refs/heads/' . a:context.branch
  if s:git(l:repository, ['rev-parse', '--symbolic-full-name', 'HEAD'])[0] ==# l:ref
    call s:fail(a:context.branch . ' is checked out here; commit suggestions with git')
  endif
  call s:unsaved(l:repository)
  let l:tip = s:git(l:repository, ['rev-parse', '--verify', l:ref . '^{commit}'])[0]
  let l:files = {'paths': tempname(), 'patch': tempname(), 'index': tempname(),
        \ 'message': tempname()}
  try
    call s:git(l:repository, ['diff', 'HEAD', '--name-only', '-z', '--no-renames',
          \ '--no-relative', '--output=' . l:files.paths])
    if getfsize(l:files.paths) <= 0
      call s:fail('no uncommitted edits to suggest')
    endif
    call s:git(l:repository, ['diff', 'HEAD', '--binary', '--full-index', '--no-renames',
          \ '--no-ext-diff', '--no-color', '--no-relative', '--src-prefix=a/',
          \ '--dst-prefix=b/', '--output=' . l:files.patch])
    call s:git_index(l:files.index, l:repository, ['read-tree', l:tip])
    try
      call s:git_index(l:files.index, l:repository,
            \ ['apply', '--cached', '--3way', l:files.patch])
    catch /^nota:/
      if !s:resolve(l:files.index, l:repository)
        call s:fail('your edits conflict with the review:'
              \ . substitute(v:exception, '^nota:', '', ''))
      endif
    endtry
    let l:tree = s:git_index(l:files.index, l:repository, ['write-tree'])[0]
    if l:tree ==# s:git(l:repository, ['rev-parse', l:tip . '^{tree}'])[0]
      call s:fail('the review already contains these edits')
    endif
    call writefile(split(l:text, "\n", 1), l:files.message)
    let l:commit = s:git(l:repository,
          \ ['commit-tree', l:tree, '-p', l:tip, '-F', l:files.message])[0]
    call s:git(l:repository, ['update-ref', l:ref, l:commit, l:tip])
    call s:git(l:repository, ['--literal-pathspecs', 'restore', '--source=HEAD',
          \ '--staged', '--worktree', '--pathspec-from-file=' . l:files.paths,
          \ '--pathspec-file-nul'])
  finally
    for l:file in values(l:files)
      call delete(l:file)
    endfor
  endtry
  call s:reload(l:repository)
  call s:inline(l:repository)
  echomsg 'Nota: ' . strpart(l:commit, 0, 12) . '  suggestion'
  return 1
endfunction

function! s:draft(kind, context, source) abort
  call s:scratch(a:kind, a:context)
  setlocal buftype=acwrite bufhidden=hide filetype=markdown
  let b:nota_kind = a:kind
  let b:nota_source = a:source
  " Context stays outside the editable text, so an empty draft is refused.
  augroup nota_note
    autocmd! * <buffer>
    autocmd BufWriteCmd <buffer> call nota#command('submit', [])
  augroup END
  nunmap <buffer> q
  echomsg 'Nota: ' . a:kind . ' for ' . a:context.branch
        \ . '; :write submits, :bdelete! discards'
  return 1
endfunction

function! s:note(message, range, first, last) abort
  let l:context = s:context()
  call s:associate(l:context)
  " Resolve the checked-out default now, so an open draft keeps its target.
  call s:load(l:context)
  " Without a range, a note from a file buffer points at the cursor line.
  let l:file = &buftype ==# '' && !empty(expand('%:p'))
  let l:source = a:range || l:file ? s:location(l:context, a:first, a:last) : {}
  if !empty(a:message)
    return s:add_note(l:context, a:message, l:source)
  endif
  return s:draft('note', l:context, l:source)
endfunction

function! s:suggest(message) abort
  let l:context = s:context()
  call s:associate(l:context)
  if empty(l:context.branch)
    call s:fail('no review started; use :NotaStart, or select one with :NotaBranch')
  endif
  call s:load(l:context)
  if !empty(a:message)
    return s:add_suggestion(l:context, a:message)
  endif
  return s:draft('suggestion', l:context, {})
endfunction

function! s:submit() abort
  let l:text = join(getline(1, '$'), "\n")
  if l:text !~# '\S'
    call s:fail('review message must not be empty')
  endif
  if b:nota_kind ==# 'suggestion'
    call s:add_suggestion(b:nota_context, l:text)
  else
    call s:add_note(b:nota_context, l:text, b:nota_source)
  endif
  setlocal nomodified
  bwipeout
  return 1
endfunction

function! s:fill(review) abort
  let [l:lines, b:nota_commits] = s:render(a:review)
  setlocal modifiable noreadonly
  call setline(1, l:lines)
  if line('$') > len(l:lines)
    execute (len(l:lines) + 1) . ',$delete _'
  endif
  setlocal nomodified nomodifiable readonly
endfunction

function! s:refresh() abort
  call s:fill(s:load(copy(b:nota_context)))
  call s:inline(b:nota_context.repository)
  return 1
endfunction

function! s:entry() abort
  let l:commit = get(b:nota_commits, line('.') - 1, '')
  if empty(l:commit)
    call s:fail('put the cursor on the subject, a note, or a suggestion')
  endif
  return s:commit(b:nota_context, l:commit)
endfunction

function! s:patch(context, commit) abort
  return s:git(a:context.repository,
        \ ['--no-pager', 'show', '--no-color', '--no-ext-diff', '--no-textconv', a:commit, '--'])
endfunction

" Show a review entry's full message and patch in a split.
function! s:commit(context, commit) abort
  let l:lines = s:patch(a:context, a:commit)
  call s:scratch('entry', a:context)
  call setline(1, l:lines)
  setlocal filetype=git nomodified nomodifiable readonly
  return 1
endfunction

" One item per location the CLI placed an entry at in the worktree files: a
" note's lines, or each hunk of a suggestion. A general note has none.
function! s:items(context, entry) abort
  let l:text = printf('[%s %s] %s', a:entry.kind, strpart(a:entry.commit, 0, 8),
        \ s:summary(a:entry))
  if empty(a:entry.locations)
    return [{'text': l:text}]
  endif
  let l:items = []
  for l:location in a:entry.locations
    let l:item = {'filename': a:context.repository . '/' . l:location.path,
          \ 'text': l:text . (index(['applied', 'stale', 'changed'], l:location.status) >= 0
          \   ? ' (' . l:location.status . ')' : '')}
    " Files changed without lines, such as binary files, are listed once.
    if l:location.line isnot v:null
      " Lines removed or yet to be added sit after `line`; point at it.
      let l:item.lnum = max([l:location.line, 1])
      let l:item.end_lnum = l:item.lnum + max([l:location.count, 1]) - 1
    endif
    call add(l:items, l:item)
  endfor
  return l:items
endfunction

function! s:quickfix(...) abort
  let l:context = s:context()
  if a:0
    let l:context.branch = a:1
  endif
  call s:associate(l:context)
  let l:review = json_decode(join(s:cli(l:context, 'show', [], ['--json', '--at=worktree']), "\n"))
  let l:context.branch = l:review.branch
  let l:items = []
  for l:entry in l:review.entries
    let l:items += s:items(l:context, l:entry)
  endfor
  call setqflist([], ' ', {'title': 'Nota ' . l:review.branch, 'items': l:items})
  if empty(l:items)
    echomsg 'Nota: ' . l:review.branch . ' has no entries'
  else
    botright copen
  endif
  return 1
endfunction
