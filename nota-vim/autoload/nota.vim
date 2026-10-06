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

function! s:cli(context, command, arguments) abort
  let l:args = [get(g:, 'nota_executable', 'nota'), a:command,
        \ '--repository=' . a:context.repository]
  if !empty(a:context.branch)
    call add(l:args, '--branch=' . a:context.branch)
  endif
  " Positional arguments after -- cannot become CLI options.
  return s:run(l:args + ['--'] + a:arguments)
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

function! s:load(context) abort
  let l:lines = s:cli(a:context, 'show', [])
  let a:context.branch = s:review_branch(l:lines)
  return l:lines
endfunction

function! s:scratch(kind, context) abort
  botright new
  let s:buffer_id += 1
  execute 'file nota://' . a:kind . '/' . s:buffer_id
  setlocal buftype=nofile bufhidden=wipe noswapfile nobuflisted
  let b:nota_context = copy(a:context)
  nnoremap <silent><buffer> q :close<CR>
endfunction

function! s:display(context, lines) abort
  call s:scratch('review', a:context)
  call setline(1, a:lines)
  setlocal filetype=nota nomodified nomodifiable readonly
  nnoremap <silent><buffer> r :call nota#command('refresh', [])<CR>
  nnoremap <silent><buffer> <CR> :call nota#command('entry', [])<CR>
  nnoremap <silent><buffer> n :NotaNote<CR>
  nnoremap <silent><buffer> s :NotaSuggest<CR>
  return 1
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
  let l:lines = s:load(l:context)
  let s:branches[l:context.repository] = l:context.branch
  return s:display(l:context, l:lines)
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
  return 1
endfunction

function! s:location(context, first, last) abort
  if &buftype !=# '' || empty(expand('%:p'))
    call s:fail('a line range needs a file buffer')
  endif
  let l:path = expand('%:p')
  let l:prefix = a:context.repository . '/'
  if stridx(l:path, l:prefix) == 0
    let l:path = strpart(l:path, strlen(l:prefix))
  endif
  let l:range = a:first == a:last ? string(a:first) : a:first . '-' . a:last
  return 'Source: ' . l:path . ':' . l:range
endfunction

function! s:add_note(context, text) abort
  if a:text !~# '\S'
    call s:fail('review message must not be empty')
  endif
  let l:lines = s:cli(a:context, 'note', [a:text])
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
      call s:fail('your edits conflict with the review:'
            \ . substitute(v:exception, '^nota:', '', ''))
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
  " Resolve the checked-out default now, so an open draft keeps its target.
  call s:load(l:context)
  let l:source = a:range ? s:location(l:context, a:first, a:last) : ''
  if !empty(a:message)
    let l:text = a:message . (empty(l:source) ? '' : "\n\n" . l:source)
    return s:add_note(l:context, l:text)
  endif
  return s:draft('note', l:context, l:source)
endfunction

function! s:suggest(message) abort
  let l:context = s:context()
  if empty(l:context.branch)
    call s:fail('no review started; use :NotaStart, or select one with :NotaBranch')
  endif
  call s:load(l:context)
  if !empty(a:message)
    return s:add_suggestion(l:context, a:message)
  endif
  return s:draft('suggestion', l:context, '')
endfunction

function! s:submit() abort
  let l:text = join(getline(1, '$'), "\n")
  if l:text !~# '\S'
    call s:fail('review message must not be empty')
  endif
  if !empty(b:nota_source)
    let l:text .= "\n\n" . b:nota_source
  endif
  if b:nota_kind ==# 'suggestion'
    call s:add_suggestion(b:nota_context, l:text)
  else
    call s:add_note(b:nota_context, l:text)
  endif
  setlocal nomodified
  bwipeout
  return 1
endfunction

function! s:refresh() abort
  let l:context = copy(b:nota_context)
  let l:lines = s:load(l:context)
  setlocal modifiable noreadonly
  call setline(1, l:lines)
  if line('$') > len(l:lines)
    execute (len(l:lines) + 1) . ',$delete _'
  endif
  setlocal nomodified nomodifiable readonly
  return 1
endfunction

function! s:entry() abort
  let l:commit = matchstr(getline('.'), '^\x\{12,64}\ze\s\+\%(note\|suggestion\)\>')
  if empty(l:commit)
    let l:commit = matchstr(getline('.'), '^subject\s\+\zs\x\{40,64}\ze\s*$')
  endif
  if empty(l:commit)
    call s:fail('put the cursor on the subject, a note, or a suggestion')
  endif
  let l:context = copy(b:nota_context)
  let l:lines = s:git(l:context.repository,
        \ ['--no-pager', 'show', '--no-color', '--no-ext-diff', '--no-textconv', l:commit, '--'])
  call s:scratch('entry', l:context)
  call setline(1, l:lines)
  setlocal filetype=git nomodified nomodifiable readonly
  return 1
endfunction
