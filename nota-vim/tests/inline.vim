" Neovim only: nvim --headless -u NONE -n -S nota-vim/tests/inline.vim
set nocompatible
set nomore
set hidden

let s:plugin = fnamemodify(expand('<sfile>:p'), ':h:h')
let s:project = fnamemodify(s:plugin, ':h')
execute 'set runtimepath^=' . fnameescape(s:plugin)
runtime plugin/nota.vim
let g:nota_executable = empty($NOTA_TEST_EXECUTABLE)
      \ ? s:project . '/target/debug/nota' : $NOTA_TEST_EXECUTABLE
let s:temporary = tempname()
let s:repository = s:temporary . '/repo'
let s:worktree = s:temporary . '/worktree'

function! s:git(repository, arguments) abort
  let l:output = system(join(map(['git', '-C', a:repository] + a:arguments,
        \ 'shellescape(v:val)'), ' ') . ' 2>&1')
  if v:shell_error
    throw 'test command failed: ' . l:output
  endif
  return substitute(l:output, '\n$', '', '')
endfunction

" Each review item in the current buffer: [line, status, summary].
function! s:items() abort
  return luaeval('vim.tbl_map(function(mark)'
        \ . ' local text = mark[4].virt_text[1][1]'
        \ . ' return { mark[2] + 1, text:match("%((%a+)%)$") or "pending",'
        \ . ' (text:gsub("^✎ %x+ ", ""):gsub(" %(%a+%)$", "")) } end,'
        \ . ' vim.tbl_filter(function(mark) return mark[4].virt_text ~= nil end,'
        \ . ' vim.api.nvim_buf_get_extmarks(0, require("nota.inline").namespace,'
        \ . ' 0, -1, { details = true })))')
endfunction

" Highlights before virtual lines on the same line; extmark order is not.
function! s:by_line(a, b) abort
  return a:a[0] != a:b[0] ? a:a[0] - a:b[0]
        \ : (a:a[1] !~# '^NotaInline') - (a:b[1] !~# '^NotaInline')
endfunction

" Line highlights and virtual lines, as [line, group] and [line, text...].
function! s:expanded() abort
  return sort(s:expanded_marks(), function('s:by_line'))
endfunction

function! s:expanded_marks() abort
  return luaeval('(function() local result = {}'
        \ . ' for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(0,'
        \ . ' require("nota.inline").namespace, 0, -1, { details = true })) do'
        \ . ' local details = mark[4]'
        \ . ' if details.line_hl_group then'
        \ . ' table.insert(result, { mark[2] + 1, details.line_hl_group }) end'
        \ . ' if details.virt_lines then local entry = { mark[2] + 1 }'
        \ . ' for _, line in ipairs(details.virt_lines) do'
        \ . ' table.insert(entry, vim.trim(line[1][1])) end'
        \ . ' table.insert(result, entry) end end return result end)()')
endfunction

try
  if !executable(g:nota_executable)
    throw 'Build the CLI first: cargo build -p nota'
  endif
  call mkdir(s:repository, 'p')
  call s:git(s:repository, ['init', '-b', 'main'])
  call s:git(s:repository, ['config', 'user.name', 'Nota Vim test'])
  call s:git(s:repository, ['config', 'user.email', 'nota-vim@example.invalid'])
  call writefile(['one', 'two', 'three', 'four', 'five', 'six', 'seven'],
        \ s:repository . '/file.txt')
  call s:git(s:repository, ['add', 'file.txt'])
  call s:git(s:repository, ['commit', '-m', 'Subject'])
  execute 'edit ' . fnameescape(s:repository . '/file.txt')

  NotaStart HEAD nota/inline
  close
  call setline(1, ['one', 'TWO', 'three', 'inserted', 'four', 'five', 'six', 'seven'])
  write
  NotaSuggest Shout and insert.
  call setline(6, "\tsix")
  write
  NotaSuggest Indent six.
  call assert_equal(['one', 'two', 'three', 'four', 'five', 'six', 'seven'], getline(1, '$'))

  " Off by default; on, each hunk is marked where it applies.
  call assert_equal([], s:items())
  NotaInline
  call assert_equal([[2, 'pending', 'Shout and insert.'], [3, 'pending', 'Shout and insert.'],
        \ [6, 'pending', 'Indent six.']], s:items())
  call assert_equal(']r', maparg(']r', 'n', 0, 1).lhs)

  " ]r and [r step between items, with counts.
  call cursor(1, 1)
  normal ]r
  call assert_equal(2, line('.'))
  normal 2]r
  call assert_equal(6, line('.'))
  normal [r
  call assert_equal(3, line('.'))

  " Expanding shows removed lines highlighted and suggested lines below.
  call cursor(2, 1)
  lua require('nota.inline').toggle_item()
  call assert_equal([[2, 'NotaInlineDelete'], [2, 'TWO']], s:expanded())
  lua require('nota.inline').toggle_all()
  call assert_equal([[2, 'NotaInlineDelete'], [2, 'TWO'], [3, 'inserted'],
        \ [6, 'NotaInlineDelete'], [6, 'six']], s:expanded())
  lua require('nota.inline').toggle_all()
  call assert_equal([], s:expanded())

  " Items follow lines added above them, and go stale when their lines change.
  call writefile(['zero', 'one', 'two', 'three', 'four', 'five', 'SIX', 'seven'],
        \ s:repository . '/file.txt')
  edit!
  call assert_equal([[3, 'pending', 'Shout and insert.'], [4, 'pending', 'Shout and insert.'],
        \ [7, 'stale', 'Indent six.']], s:items())
  call s:git(s:repository, ['checkout', '--', 'file.txt'])
  edit!

  " In a worktree of the review, the suggestions are already applied.
  call s:git(s:repository, ['worktree', 'add', s:worktree, 'nota/inline'])
  execute 'edit ' . fnameescape(s:worktree . '/file.txt')
  call assert_equal([[2, 'applied', 'Shout and insert.'], [4, 'applied', 'Shout and insert.'],
        \ [7, 'applied', 'Indent six.']], s:items())
  lua require('nota.inline').toggle_all()
  call assert_equal([[2, 'NotaInlineAdd'], [2, 'two'], [4, 'NotaInlineAdd'],
        \ [7, 'NotaInlineAdd'], [7, 'six']], s:expanded())

  " Turning the display off clears items and mappings.
  NotaInline
  call assert_equal([], s:items())
  call assert_equal({}, maparg(']r', 'n', 0, 1))
catch
  call add(v:errors, v:exception . ' at ' . v:throwpoint)
finally
  call delete(s:temporary, 'rf')
endtry

if !empty(v:errors)
  for s:error in v:errors
    echomsg s:error
  endfor
  cquit
endif
qa!
