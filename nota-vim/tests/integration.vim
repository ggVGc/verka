set nocompatible
set nomore
set hidden

let s:plugin = fnamemodify(expand('<sfile>:p'), ':h:h')
let s:project = fnamemodify(s:plugin, ':h')
execute 'set runtimepath^=' . fnameescape(s:plugin)
runtime plugin/nota.vim
let g:nota_executable = empty($NOTA_TEST_EXECUTABLE)
      \ ? s:project . '/target/debug/nota' : $NOTA_TEST_EXECUTABLE
let s:executable = g:nota_executable
let s:temporary = tempname()
let s:repository = s:temporary . "/review repo's % # !"
let s:other = s:temporary . '/other'
let s:worktree = s:temporary . '/worktree'
let s:original_directory = getcwd()

function! s:run(arguments) abort
  let l:output = system(join(map(copy(a:arguments), 'shellescape(v:val)'), ' ') . ' 2>&1')
  if v:shell_error
    throw 'test command failed: ' . l:output
  endif
  return substitute(l:output, '\n$', '', '')
endfunction

function! s:git(repository, arguments) abort
  return s:run(['git', '-C', a:repository] + a:arguments)
endfunction

function! s:fixture(repository) abort
  call mkdir(a:repository, 'p')
  call s:git(a:repository, ['init', '-b', 'main'])
  call s:git(a:repository, ['config', 'user.name', 'Nota Vim test'])
  call s:git(a:repository, ['config', 'user.email', 'nota-vim@example.invalid'])
  call writefile(['one', 'two', 'three'], a:repository . '/file.txt')
  call s:git(a:repository, ['add', 'file.txt'])
  call s:git(a:repository, ['commit', '-m', 'Subject'])
endfunction

function! s:edit(repository) abort
  execute 'edit ' . fnameescape(a:repository . '/file.txt')
endfunction

function! s:message(repository, branch) abort
  return s:git(a:repository, ['show', '-s', '--format=%B', a:branch])
endfunction

try
  if !executable(g:nota_executable)
    throw 'Build the CLI first: cargo build -p nota'
  endif
  call s:fixture(s:repository)
  call s:fixture(s:other)
  " Run from another directory: commands must follow the file, not :pwd.
  execute 'cd ' . fnameescape(s:other)
  call s:edit(s:repository)
  call writefile(['staged change'], s:repository . '/staged.txt')
  call s:git(s:repository, ['add', 'staged.txt'])
  let s:status = s:git(s:repository, ['status', '--porcelain'])
  let s:index = s:git(s:repository, ['write-tree'])
  let s:subject = s:git(s:repository, ['rev-parse', 'HEAD'])

  NotaStart HEAD nota/first
  call assert_equal('nota/first', b:nota_context.branch)
  call assert_equal(s:repository, b:nota_context.repository)
  call assert_equal('nofile', &buftype)
  call assert_false(&modifiable)
  call assert_match('entries  none', join(getline(1, '$'), "\n"))
  call cursor(2, 1)
  execute "normal \<CR>"
  call assert_equal('git', &filetype)
  call assert_match('Subject', join(getline(1, '$'), "\n"))
  normal q
  call assert_equal('main', s:git(s:repository, ['branch', '--show-current']))
  call assert_equal(s:subject, s:git(s:repository, ['rev-parse', 'HEAD']))
  close

  " Shell metacharacters, Ex separators, and leading CLI options are prose.
  let s:text = "--literal % # ! 'quoted' \"double\"; $(touch injected) `touch injected` | more"
  call cursor(1, 1)
  execute 'NotaNote ' . s:text
  call assert_match('\V' . escape(s:text, '\'), s:message(s:repository, 'nota/first'))
  " Without a range, a note from a file buffer records the cursor line.
  call assert_match('\nNota-Source: \x\{40}:file.txt:1-1\n', s:message(s:repository, 'nota/first'))
  call assert_false(filereadable(s:other . '/injected'))
  call assert_false(filereadable(s:repository . '/injected'))
  1,2NotaNote Explain these lines.
  call assert_match('Nota-Source: \x\{40}:file.txt:1-2', s:message(s:repository, 'nota/first'))
  call assert_equal(s:status, s:git(s:repository, ['status', '--porcelain']))
  call assert_equal(s:index, s:git(s:repository, ['write-tree']))

  " A draft retains its repository, selected branch, source, and prose.
  2,3NotaNote
  let s:draft = bufnr('%')
  call assert_equal('acwrite', &buftype)
  call assert_equal(['file.txt', 2, 3],
        \ [b:nota_source.path, b:nota_source.first, b:nota_source.last])
  let s:tip = s:git(s:repository, ['rev-parse', 'nota/first'])
  write
  call assert_equal(s:draft, bufnr('%'))
  call assert_equal(s:tip, s:git(s:repository, ['rev-parse', 'nota/first']))
  call setline(1, ['Multiline note', '', 'Details: % # ! and quotes '' "'])
  let g:nota_executable = s:temporary . '/missing-nota'
  write
  call assert_equal(s:draft, bufnr('%'))
  call assert_true(&modified)
  call assert_equal('Multiline note', getline(1))
  let g:nota_executable = s:executable
  call s:edit(s:repository)
  NotaStart HEAD nota/second
  close
  execute 'buffer ' . s:draft
  write
  call assert_false(bufexists(s:draft))
  let s:message = s:message(s:repository, 'nota/first')
  call assert_match('Multiline note\n\nDetails:', s:message)
  call assert_match('Nota-Source: \x\{40}:file.txt:2-3', s:message)
  call assert_match('Start review', s:message(s:repository, 'nota/second'))

  call s:edit(s:repository)
  NotaShow nota/first
  call assert_match('Multiline note', join(getline(1, '$'), "\n"))
  call search('Multiline note')
  call assert_equal(1, nota#command('entry', []))
  call assert_equal('git', &filetype)
  call assert_match('Nota-Source: \x\{40}:file.txt:2-3', join(getline(1, '$'), "\n"))
  close
  NotaNote Added from the review buffer.
  normal r
  call assert_match('Added from the review buffer', join(getline(1, '$'), "\n"))
  close

  " Invalid reviews and failed starts preserve the previous selection.
  call assert_equal(0, nota#command('branch', ['main']))
  call assert_equal(0, nota#command('start', ['missing-revision', 'nota/bad']))
  call assert_equal(0, nota#command('start', ['HEAD', 'nota/first']))
  NotaNote Still on the first review.
  call assert_match('Still on the first review', s:message(s:repository, 'nota/first'))

  " A second repository has an independent selection, including generated names.
  call s:edit(s:other)
  NotaStart
  let s:other_branch = b:nota_context.branch
  call assert_match('^nota/review-', s:other_branch)
  close
  NotaNote Other repository.
  call assert_match('Other repository', s:message(s:other, s:other_branch))
  call s:edit(s:repository)
  NotaNote Original repository.
  call assert_match('Original repository', s:message(s:repository, 'nota/first'))

  " Suggestions use normal commits in a linked worktree.
  call s:git(s:repository, ['worktree', 'add', s:worktree, 'nota/first'])
  call writefile(['suggested', 'two', 'three'], s:worktree . '/file.txt')
  call s:git(s:worktree, ['add', 'file.txt'])
  call s:git(s:worktree, ['commit', '-m', 'Improve the first line'])
  call s:edit(s:worktree)
  " No selection here: use the checked-out review branch.
  NotaShow
  call assert_equal(s:worktree, b:nota_context.repository)
  call assert_equal('nota/first', b:nota_context.branch)
  call search('suggestion')
  call assert_equal(1, nota#command('entry', []))
  call assert_match('+suggested', join(getline(1, '$'), "\n"))
  close
  close

  " Clearing selection uses checked-out HEAD, and drafts pin that default too.
  NotaBranch
  NotaNote
  let s:draft = bufnr('%')
  call setline(1, 'Pinned checked-out review.')
  call s:git(s:worktree, ['checkout', '--detach', s:subject])
  write
  call assert_false(bufexists(s:draft))
  call assert_match('Pinned checked-out review', s:message(s:repository, 'nota/first'))
  call assert_equal(0, nota#command('show', []))
  call s:edit(s:repository)
  NotaBranch
  call assert_equal(0, nota#command('show', []))

  " Suggestions move uncommitted edits to the selected review only.
  call setline(1, ['one', 'two', 'THREE'])
  write
  call assert_equal(0, nota#command('suggest', ['No review selected.']))
  NotaBranch nota/first
  let s:tip = s:git(s:repository, ['rev-parse', 'nota/first'])
  " The review changed the first line; a different change to it conflicts.
  call setline(1, 'ONE')
  write
  call assert_equal(0, nota#command('suggest', ['Conflicts with the review.']))
  call setline(1, 'one')
  call assert_equal(0, nota#command('suggest', ['Unsaved edits.']))
  call assert_equal(s:tip, s:git(s:repository, ['rev-parse', 'nota/first']))
  write
  NotaSuggest Shout the last line.
  call assert_match('Shout the last line', s:message(s:repository, 'nota/first'))
  call assert_equal("file.txt\nstaged.txt",
        \ s:git(s:repository, ['diff-tree', '--name-only', '-r', '--no-commit-id', 'nota/first']))
  call assert_equal(['suggested', 'two', 'THREE'],
        \ split(s:git(s:repository, ['show', 'nota/first:file.txt']), "\n"))
  " The suggested edits are undone in the checkout, index, and buffer.
  call assert_equal('', s:git(s:repository, ['status', '--porcelain']))
  call assert_equal(['one', 'two', 'three'], getline(1, '$'))
  call assert_false(&modified)
  call assert_equal(0, nota#command('suggest', ['Nothing new.']))
  " Later suggestions carry only their own edits.
  call writefile(['added'], s:repository . '/added.txt')
  call s:git(s:repository, ['add', 'added.txt'])
  NotaSuggest
  call assert_equal('acwrite', &buftype)
  call setline(1, ['Shout the middle line', '', 'Details.'])
  write
  call assert_match('Shout the middle line\n\nDetails.', s:message(s:repository, 'nota/first'))
  call assert_equal('added.txt',
        \ s:git(s:repository, ['diff-tree', '--name-only', '-r', '--no-commit-id', 'nota/first']))
  call assert_equal(['suggested', 'two', 'THREE'],
        \ split(s:git(s:repository, ['show', 'nota/first:file.txt']), "\n"))
  call assert_false(filereadable(s:repository . '/added.txt'))
  call assert_equal('', s:git(s:repository, ['status', '--porcelain']))
  NotaShow
  call assert_match('suggestion Shout the middle line', join(getline(1, '$'), "\n"))
  call search('added.txt')
  call assert_equal(1, nota#command('entry', []))
  call assert_match('+added', join(getline(1, '$'), "\n"))
  close

  " The quickfix list holds every entry, at its source lines or hunks.
  normal gq
  call assert_equal('qf', &filetype)
  call assert_equal('Nota nota/first', getqflist({'title': 1}).title)
  let s:items = map(getqflist(),
        \ '[v:val.bufnr ? bufname(v:val.bufnr) : "", v:val.lnum, v:val.text]')
  call map(s:items, '[fnamemodify(v:val[0], ":t"), v:val[1], v:val[2]]')
  call assert_equal(['file.txt', 1, '[note '], [s:items[1][0], s:items[1][1], s:items[1][2][:5]])
  call assert_match('Explain these lines', s:items[1][2])
  call assert_equal(['file.txt', 1], s:items[0][:1])
  call assert_equal(['file.txt', 2], s:items[2][:1])
  call assert_match('^\[note \x\{8}\] Multiline note$', s:items[2][2])
  " A note from the review buffer is general, without a location.
  call assert_equal([['', 0]], map(filter(copy(s:items),
        \ 'v:val[2] =~# "Added from the review buffer"'), 'v:val[:1]'))
  let s:suggestions = filter(copy(s:items), 'v:val[2] =~# "^\\[suggestion"')
  call assert_equal([['file.txt', 1], ['file.txt', 3], ['staged.txt', 1], ['added.txt', 1]],
        \ map(copy(s:suggestions), 'v:val[:1]'))
  call assert_match('Shout the last line', s:suggestions[2][2])
  cclose
  close
  enew
  2cc
  call assert_equal(s:repository . '/file.txt', expand('%:p'))
  call assert_equal(1, line('.'))
  cnext
  call assert_equal(2, line('.'))
  NotaBranch
  let s:status = s:git(s:repository, ['status', '--porcelain'])
  let s:index = s:git(s:repository, ['write-tree'])
  " Unnamed buffers use :pwd.
  enew
  execute 'cd ' . fnameescape(s:repository)
  NotaShow nota/second
  call assert_equal(s:repository, b:nota_context.repository)
  close
  call assert_equal(s:status, s:git(s:repository, ['status', '--porcelain']))
  call assert_equal(s:index, s:git(s:repository, ['write-tree']))

  " Without a selection, commands select the review of the checked-out branch.
  call s:run([s:executable, 'start', '--repository=' . s:repository, 'HEAD'])
  call s:git(s:repository, ['branch', 'main-3'])
  call s:run([s:executable, 'start', '--repository=' . s:repository, 'main-3'])
  call s:edit(s:repository)
  NotaBranch
  NotaNote Found the branch's review.
  call assert_equal('nota/review-main', nota#selected(s:repository))
  call assert_match('Found the branch', s:message(s:repository, 'nota/review-main'))
  call assert_match('selected nota/review-main, the review of main',
        \ execute('messages'))
  " With several, ask which; nota/review-main-3 belongs to main-3.
  call s:run([s:executable, 'start', '--repository=' . s:repository, 'HEAD'])
  " Vim's Ex mode reads inputlist() from stdin rather than typeahead.
  if has('nvim')
    NotaBranch
    call feedkeys("2\<CR>", 't')
    NotaQuickfix
    call assert_equal('Nota nota/review-main-2', getqflist({'title': 1}).title)
    call assert_equal('nota/review-main-2', nota#selected(s:repository))
    call feedkeys("\<CR>", 't')
  endif
  NotaBranch
  call assert_equal(0, nota#command('show', []))
  call assert_equal('', nota#selected(s:repository))
catch
  call add(v:errors, v:exception . ' at ' . v:throwpoint)
finally
  execute 'cd ' . fnameescape(s:original_directory)
  call delete(s:temporary, 'rf')
endtry

if !empty(v:errors)
  for s:error in v:errors
    echomsg s:error
  endfor
  cquit
endif
qa!
