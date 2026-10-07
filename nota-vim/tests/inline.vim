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
  return sort(s:item_marks(), function('s:by_line'))
endfunction

function! s:item_marks() abort
  return luaeval('vim.tbl_map(function(mark)'
        \ . ' local text = mark[4].virt_text[1][1]'
        \ . ' return { mark[2] + 1, text:match("%((%a+)%)$")'
        \ . ' or (text:find("^●") and "note" or "pending"),'
        \ . ' (text:gsub("^%S+ %x+ ", ""):gsub(" %(%a+%)$", "")) } end,'
        \ . ' vim.tbl_filter(function(mark) return mark[4].virt_text ~= nil end,'
        \ . ' vim.api.nvim_buf_get_extmarks(0, require("nota.inline").namespace,'
        \ . ' 0, -1, { details = true })))')
endfunction

" The sign group at the first line of each item: [line, group].
function! s:signs() abort
  return luaeval('vim.tbl_map(function(mark)'
        \ . ' return { mark[2] + 1, mark[4].sign_hl_group } end,'
        \ . ' vim.tbl_filter(function(mark) return mark[4].virt_text ~= nil end,'
        \ . ' vim.api.nvim_buf_get_extmarks(0, require("nota.inline").namespace,'
        \ . ' 0, -1, { details = true })))')
endfunction

" By line, highlights first, then by text; extmark order is not stable.
function! s:by_line(a, b) abort
  if a:a[0] != a:b[0]
    return a:a[0] - a:b[0]
  endif
  let l:group = (a:a[1] !~# '^NotaInline') - (a:b[1] !~# '^NotaInline')
  return l:group ? l:group : string(a:a) ># string(a:b) ? 1 : -1
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
  call writefile(['a', 'b', 'c'], s:repository . '/other.txt')
  call s:git(s:repository, ['add', 'file.txt', 'other.txt'])
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
  execute 'edit ' . fnameescape(s:repository . '/other.txt')
  2delete
  write
  NotaSuggest Drop b.
  execute 'edit ' . fnameescape(s:repository . '/file.txt')
  4,5NotaNote Explain these lines.
  " Notes from the review buffer are general and have no place in a file.
  NotaShow nota/inline
  " Notes from the review buffer are general and have no place in a file.
  NotaShow
  NotaNote A general note.
  close
  close
  call assert_equal(['one', 'two', 'three', 'four', 'five', 'six', 'seven'], getline(1, '$'))

  " Quickfix items are at the lines this checkout has, though the second
  " suggestion's line 7 comes after the first one's insertion.
  NotaQuickfix
  call assert_equal([['file.txt', 2, '[suggestion'], ['file.txt', 3, '[suggestion'],
        \ ['file.txt', 6, '[suggestion'], ['other.txt', 2, '[suggestion'],
        \ ['file.txt', 4, '[note'], ['', 0, '[note']],
        \ map(getqflist(), '[v:val.bufnr ? fnamemodify(bufname(v:val.bufnr), ":t") : "", v:val.lnum,'
        \ . ' matchstr(v:val.text, "^\\S\\+")]'))
  call assert_equal(5, getqflist()[4].end_lnum)
  cclose

  " Off by default; on, each hunk and located note is marked where it applies.
  call assert_equal([], s:items())
  NotaInline
  call assert_equal([[2, 'pending', 'Shout and insert.'], [3, 'pending', 'Shout and insert.'],
        \ [4, 'note', 'Explain these lines.'], [6, 'pending', 'Indent six.']], s:items())
  call assert_equal(']r', maparg(']r', 'n', 0, 1).lhs)

  " Signs are coloured by whether a hunk adds, removes, or replaces lines.
  call assert_equal([[2, 'NotaInlineSignChange'], [3, 'NotaInlineSignAdd'],
        \ [4, 'NotaInlineNote'], [6, 'NotaInlineSignChange']], sort(s:signs()))
  execute 'edit ' . fnameescape(s:repository . '/other.txt')
  call assert_equal([[2, 'pending', 'Drop b.']], s:items())
  call assert_equal([[2, 'NotaInlineSignDelete']], s:signs())
  execute 'edit ' . fnameescape(s:repository . '/file.txt')

  " ]r and [r step between items, with counts.
  call cursor(1, 1)
  normal ]r
  call assert_equal(2, line('.'))
  normal 2]r
  call assert_equal(4, line('.'))
  normal [r
  call assert_equal(3, line('.'))

  " Expanding shows removed lines highlighted, suggested lines below them,
  " and a note's text below its lines.
  call cursor(2, 1)
  lua require('nota.inline').toggle_item()
  call assert_equal([[2, 'NotaInlineDelete'], [2, 'TWO']], s:expanded())
  lua require('nota.inline').toggle_all()
  call assert_equal([[2, 'NotaInlineDelete'], [2, 'TWO'], [3, 'inserted'],
        \ [5, '│ Explain these lines.'], [6, 'NotaInlineDelete'], [6, 'six']], s:expanded())
  lua require('nota.inline').toggle_all()
  call assert_equal([], s:expanded())

  " <Leader>rd opens the full commit of the entry under the cursor.
  call cursor(2, 1)
  execute "normal \\rd"
  call assert_equal('git', &filetype)
  call assert_match('Shout and insert\.\_.*+TWO', join(getline(1, '$'), "\n"))
  close
  call cursor(5, 1)
  lua require('nota.inline').show_item()
  call assert_match('Explain these lines\.', join(getline(1, '$'), "\n"))
  close
  " Where entries overlap, choose one. New notes appear at once.
  call cursor(2, 1)
  NotaNote On the shouted line.
  lua vim.ui.select = function(items, _, choose)
        \ vim.g.nota_choices = vim.tbl_map(function(item) return item.label end, items)
        \ choose(vim.tbl_filter(function(item) return item.label:find('^note') end, items)[1])
        \ end
  lua require('nota.inline').show_item()
  call assert_equal(['note On the shouted line.', 'suggestion Shout and insert.'],
        \ sort(copy(g:nota_choices)))
  call assert_match('On the shouted line\.', join(getline(1, '$'), "\n"))
  close

  " Items follow lines added above them, and go stale when their lines change.
  call writefile(['zero', 'one', 'two', 'three', 'four', 'FIVE', 'SIX', 'seven'],
        \ s:repository . '/file.txt')
  edit!
  call assert_equal([[3, 'note', 'On the shouted line.'], [3, 'pending', 'Shout and insert.'],
        \ [4, 'pending', 'Shout and insert.'], [5, 'changed', 'Explain these lines.'],
        \ [7, 'stale', 'Indent six.']], s:items())
  call s:git(s:repository, ['checkout', '--', 'file.txt'])
  edit!

  " Unsaved edits move items too, and make the lines they change stale.
  call append(0, 'unsaved')
  call setline(3, 'Two')
  doautocmd TextChanged
  call assert_equal([[3, 'note', 'On the shouted line.'], [3, 'stale', 'Shout and insert.'],
        \ [4, 'pending', 'Shout and insert.'], [5, 'note', 'Explain these lines.'],
        \ [7, 'pending', 'Indent six.']], s:items())
  edit!

  " In a worktree of the review, the suggestions are already applied.
  call s:git(s:repository, ['worktree', 'add', s:worktree, 'nota/inline'])
  execute 'edit ' . fnameescape(s:worktree . '/file.txt')
  " A note on a line a suggestion changed no longer matches there.
  call assert_equal([[2, 'applied', 'Shout and insert.'], [2, 'changed', 'On the shouted line.'],
        \ [4, 'applied', 'Shout and insert.'], [5, 'note', 'Explain these lines.'],
        \ [7, 'applied', 'Indent six.']], s:items())
  NotaQuickfix
  cclose
  call assert_equal(['[suggestion', '(applied)'],
        \ split(getqflist()[0].text)[:0] + split(getqflist()[0].text)[-1:])
  execute 'edit ' . fnameescape(s:worktree . '/other.txt')
  call assert_equal([[2, 'applied', 'Drop b.']], s:items())
  call assert_equal([[2, 'NotaInlineSignDelete']], s:signs())
  execute 'edit ' . fnameescape(s:worktree . '/file.txt')
  lua require('nota.inline').toggle_all()
  call assert_equal([[2, 'NotaInlineAdd'], [2, 'two'], [2, '│ On the shouted line.'],
        \ [4, 'NotaInlineAdd'],
        \ [6, '│ Explain these lines.'], [7, 'NotaInlineAdd'], [7, 'six']], s:expanded())

  lua require('nota.inline').toggle_all()

  " A note written here, after the suggestions, is shown in the subject's
  " checkout at the line it was about there.
  call cursor(5, 1)
  NotaNote From the review worktree.
  execute 'edit ' . fnameescape(s:repository . '/file.txt')
  call assert_equal([[4, 'note', 'Explain these lines.'], [4, 'note', 'From the review worktree.']],
        \ filter(s:items(), 'v:val[1] ==# "note" && v:val[0] == 4'))

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
