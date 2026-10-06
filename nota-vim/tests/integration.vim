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
  execute 'NotaNote ' . s:text
  call assert_match('\V' . escape(s:text, '\'), s:message(s:repository, 'nota/first'))
  call assert_false(filereadable(s:other . '/injected'))
  call assert_false(filereadable(s:repository . '/injected'))
  1,2NotaNote Explain these lines.
  call assert_match('Source: file.txt:1-2', s:message(s:repository, 'nota/first'))
  call assert_equal(s:status, s:git(s:repository, ['status', '--porcelain']))
  call assert_equal(s:index, s:git(s:repository, ['write-tree']))

  " A draft retains its repository, selected branch, source, and prose.
  2,3NotaNote
  let s:draft = bufnr('%')
  call assert_equal('acwrite', &buftype)
  call assert_equal('Source: file.txt:2-3', b:nota_source)
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
  call assert_match('Source: file.txt:2-3', s:message)
  call assert_match('Start review', s:message(s:repository, 'nota/second'))

  call s:edit(s:repository)
  NotaShow nota/first
  call assert_match('Multiline note', join(getline(1, '$'), "\n"))
  call search('Multiline note')
  call assert_equal(1, nota#command('entry', []))
  call assert_equal('git', &filetype)
  call assert_match('Source: file.txt:2-3', join(getline(1, '$'), "\n"))
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

  " Suggestions commit uncommitted edits to the selected review only.
  call setline(1, ['one', 'two', 'THREE'])
  write
  call assert_equal(0, nota#command('suggest', ['No review selected.']))
  NotaBranch nota/first
  let s:tip = s:git(s:repository, ['rev-parse', 'nota/first'])
  " The review changed the first line; edits without it would undo that.
  call assert_equal(0, nota#command('suggest', ['Undoes the review.']))
  call setline(1, 'suggested')
  call assert_equal(0, nota#command('suggest', ['Unsaved edits.']))
  call assert_equal(s:tip, s:git(s:repository, ['rev-parse', 'nota/first']))
  write
  NotaSuggest Shout the last line.
  call assert_match('Shout the last line', s:message(s:repository, 'nota/first'))
  call assert_equal("file.txt\nstaged.txt",
        \ s:git(s:repository, ['diff-tree', '--name-only', '-r', '--no-commit-id', 'nota/first']))
  call assert_equal(0, nota#command('suggest', ['Nothing new.']))
  " Edits kept after a suggestion are not suggested again.
  call setline(2, 'TWO')
  write
  NotaSuggest
  call assert_equal('acwrite', &buftype)
  call setline(1, ['Shout the middle line', '', 'Details.'])
  write
  call assert_match('Shout the middle line\n\nDetails.', s:message(s:repository, 'nota/first'))
  call assert_equal("-two\n+TWO", s:git(s:repository,
        \ ['diff', '-U0', 'nota/first~', 'nota/first', '--', 'file.txt'])->split("\n")[-2:]->join("\n"))
  call assert_equal(['suggested', 'TWO', 'THREE'],
        \ split(s:git(s:repository, ['show', 'nota/first:file.txt']), "\n"))
  NotaShow
  call assert_match('suggestion Shout the middle line', join(getline(1, '$'), "\n"))
  close
  call setline(1, ['one', 'two', 'three'])
  write
  NotaBranch
  " Unnamed buffers use :pwd.
  enew
  execute 'cd ' . fnameescape(s:repository)
  NotaShow nota/second
  call assert_equal(s:repository, b:nota_context.repository)
  close
  call assert_equal(s:status, s:git(s:repository, ['status', '--porcelain']))
  call assert_equal(s:index, s:git(s:repository, ['write-tree']))
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
