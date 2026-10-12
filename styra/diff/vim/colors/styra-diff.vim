" Vim color file
" Name:        styra-diff
" Description: Diffs as `styra-diff` draws them.
"
" Every value comes from styra-ui: `palette.rs` names the colors, `theme.rs`
" assigns them roles, and `diff.rs` styles a diff's rows with those roles,
" fading some and tinting others. The faded and tinted colors here are the
" results of `faded_style` and `blended_style` from `event_list.rs`, computed
" the same way:
"
"   metadata, context  TEXT faded 4 tenths toward NEAR_BLACK     #A0A0A0
"   hunk header        ACCENT faded 4 tenths                     #888858
"   removed code       TEXT faded 3, then 4 toward RED_FG        #D3A5A5
"
" The colors are exact with 'termguicolors'. The xterm-256 fallbacks are the
" nearest entries. The two row bands have no near equivalent there, so they
" use a dark green and a dark red instead.
"
" styra-diff draws the sign of a change and the code of a file without a
" grammar in the terminal's own green and red. Those have no RGB value, so
" with 'termguicolors' they come from g:styra_diff_green and g:styra_diff_red
" (xterm's defaults unless set). Without it they are exact.
"
" styra-diff highlights the code in a diff in its file's language, and
" tints each row's colors by its kind. Neovim does the same, with tree-sitter,
" in lua/styra_diff.lua. Code in a language with no parser installed, and
" every diff in Vim, whose diff syntax cannot tell languages apart, keep
" plain colors instead: those styra-diff gives a file it has no grammar for,
" or with g:styra_diff_highlighted set to 1, those it gives plain text in a
" file it does highlight.
"
" Where Vim cannot follow styra-diff:
"   - styra-diff dims removed lines' red; Vim has no dim attribute.
"   - The line numbers and the `│` gutter styra-diff draws are not in the
"     file, so they are not here.
"
" The bands behind changed lines reach the window's edge, as in styra-diff,
" because they are signs with a line highlight. In Vim a placed sign opens
" the sign column when 'signcolumn' is "auto"; Neovim does not open it for a
" sign without text.

hi clear
if exists('syntax_on')
  syntax reset
endif
set background=dark
let g:colors_name = 'styra-diff'
let s:cpo = &cpo
set cpo&vim

let s:green = get(g:, 'styra_diff_green', '#00CD00')
let s:red = get(g:, 'styra_diff_red', '#CD0000')
let s:highlighted = get(g:, 'styra_diff_highlighted', 0)

" Ordinary text: theme::TEXT on the terminal's own background.
hi Normal       guifg=#FFFFFF guibg=NONE ctermfg=231 ctermbg=NONE
hi SignColumn   guibg=NONE ctermbg=NONE
" theme::INACTIVE, the color of styra-diff's line numbers.
hi LineNr       guifg=#B2A896 ctermfg=144

" Code, as Styra's TextMate theme (theme::MARKDOWN_CODE_THEME) colors it.
hi Comment      guifg=#A8A8A8 ctermfg=248
hi String       guifg=#FFD7AF ctermfg=223
hi Constant     guifg=#FFD7AF ctermfg=223
hi Statement    guifg=#D7AF87 ctermfg=180
hi Operator     guifg=#FF87AF ctermfg=211
hi SpecialChar  guifg=#FF87AF ctermfg=211
hi PreProc      guifg=#FFFF87 ctermfg=228
hi Function     guifg=#FFD7D7 ctermfg=224
hi Type         guifg=#D7D787 ctermfg=186
hi Identifier   guifg=#FFFFD7 ctermfg=230
hi Error        guifg=#AA0000 guibg=NONE ctermfg=124 ctermbg=NONE
" palette::PINK, orgone's `Special`.
hi Special      guifg=#FF87AF ctermfg=211
for s:group in ['Keyword', 'Conditional', 'Repeat', 'Label', 'Exception', 'StorageClass']
  exe 'hi! link' s:group 'Statement'
endfor
unlet s:group

" Tree-sitter's captures, mapped to the TextMate scopes the theme colors:
" a definition is `entity.name.function`, a call `meta.function-call`, the
" keywords that make types and functions `storage.type`. Punctuation and
" the rest have no scope there, so they are plain text.
if has('nvim')
  let s:captures = {
        \ '#A8A8A8': ['@comment'],
        \ '#FFD7AF': ['@string', '@character', '@number', '@boolean',
        \             '@constant', '@constant.builtin'],
        \ '#D7AF87': ['@keyword'],
        \ '#FF87AF': ['@keyword.operator', '@operator', '@string.escape'],
        \ '#FFFF87': ['@keyword.import', '@keyword.directive'],
        \ '#FFD7D7': ['@function', '@function.method'],
        \ '#D7B2BC': ['@function.call', '@function.method.call',
        \             '@function.builtin'],
        \ '#D7D787': ['@type', '@type.builtin', '@keyword.function',
        \             '@keyword.type'],
        \ '#FFFFD7': ['@variable', '@variable.builtin', '@variable.parameter',
        \             '@variable.member', '@property', '@tag'],
        \ '#FFFFFF': ['@punctuation', '@punctuation.special', '@constructor',
        \             '@module', '@module.builtin', '@label', '@attribute',
        \             '@string.special', '@string.regexp', '@tag.attribute',
        \             '@tag.delimiter'],
        \ }
  for [s:color, s:groups] in items(s:captures)
    for s:group in s:groups
      exe 'hi' s:group 'guifg=' . s:color
    endfor
  endfor
  unlet s:captures s:color s:groups s:group
endif

" `diff --git` lines: theme::ACCENT, at full brightness.
hi diffFile     guifg=#D7D787 ctermfg=186
" `---` and `+++` lines: theme::TEXT, also at full brightness.
hi diffOldFile  guifg=#FFFFFF ctermfg=231
hi diffNewFile  guifg=#FFFFFF ctermfg=231
" Hunk headers, function context included: theme::ACCENT, faded.
hi diffLine     guifg=#888858 ctermfg=101
hi diffSubname  guifg=#888858 ctermfg=101
" Everything else that is not a change: theme::TEXT, faded.
hi styraDiffText guifg=#A0A0A0 ctermfg=247

" The sign of a change: the terminal's green or red, bold.
exe 'hi styraDiffAddedSign   gui=bold cterm=bold ctermfg=2 guifg=' . s:green
exe 'hi styraDiffRemovedSign gui=bold cterm=bold ctermfg=1 guifg=' . s:red
if s:highlighted
  hi diffAdded   guifg=#FFFFFF ctermfg=231
  hi diffRemoved guifg=#D3A5A5 ctermfg=181
else
  exe 'hi diffAdded   ctermfg=2 guifg=' . s:green
  exe 'hi diffRemoved ctermfg=1 guifg=' . s:red
endif
hi! link Added   diffAdded
hi! link Removed diffRemoved

" theme::DIFF_ADDED_BACKGROUND and theme::DIFF_REMOVED_BACKGROUND, behind
" each changed row out to the window's edge.
hi styraDiffAddedLine   guibg=#16261A ctermbg=22
hi styraDiffRemovedLine guibg=#2A1618 ctermbg=52

" Side by side. styra-diff has no view of a changed line on both sides.
" In Neovim, lua/styra_diff.lua draws each side as styra-diff draws one
" side of a change: the oldest side's changed rows as removed rows, the
" others' as added rows, and unchanged rows faded. Those bands replace
" Vim's own, and filler rows are barely there. In Vim these are the two
" bands and quiet grays.
if has('nvim')
  for s:group in ['DiffAdd', 'DiffChange', 'DiffText']
    exe 'hi' s:group 'guifg=NONE guibg=NONE gui=NONE ctermfg=NONE ctermbg=NONE cterm=NONE'
  endfor
  unlet s:group
  hi DiffDelete   guifg=#3A3A3A guibg=NONE ctermfg=237 ctermbg=NONE
else
  hi DiffAdd      guifg=NONE guibg=#16261A ctermfg=NONE ctermbg=22
  hi DiffDelete   guifg=#D3A5A5 guibg=#2A1618 ctermfg=181 ctermbg=52
  hi DiffChange   guifg=NONE guibg=#1C1C1C ctermfg=NONE ctermbg=234
  hi DiffText     guifg=NONE guibg=#303030 ctermfg=NONE ctermbg=236 gui=NONE cterm=NONE
endif

" Vim's diff syntax names fewer kinds of row than styra-diff colors, and
" divides some differently. These rules follow `diff_body_lines` in diff.rs.
" A later match takes priority over an earlier one at the same position, so
" each rule here overrides the ones before it, including Vim's own.
function! s:syntax() abort
  " In a `git` buffer the diff is a region within the log.
  let in = &syntax ==# 'git' ? ' contained containedin=gitDiff' : ''
  exe 'syn match styraDiffText "^.*"' . in
  exe 'syn match diffRemoved "^-.*"' . in
  exe 'syn match diffAdded "^+.*"' . in
  " Only a marker as git writes one names a file. Others are code.
  exe 'syn match diffOldFile "^--- \%(a/\|/dev/null\).*"' . in
  exe 'syn match diffNewFile "^+++ \%(b/\|/dev/null\).*"' . in
  exe 'syn match diffFile "^diff --git .*"' . in
  exe 'syn match diffLine "^@@.*"' . in
  syn match styraDiffAddedSign "^+" contained containedin=diffAdded
  syn match styraDiffRemovedSign "^-" contained containedin=diffRemoved
endfunction

sign define styraDiffAdded linehl=styraDiffAddedLine
sign define styraDiffRemoved linehl=styraDiffRemovedLine

" Band each change: a line beginning `+` or `-` that is no file marker.
function! s:bands() abort
  let buf = bufnr()
  call sign_unplace('styra_diff', {'buffer': buf})
  let signs = []
  let lnum = 0
  for line in getline(1, '$')
    let lnum += 1
    if line =~# '^[-+]' && line !~# '^\%(--- \%(a/\|/dev/null\)\|+++ \%(b/\|/dev/null\)\)'
      call add(signs, {'buffer': buf, 'group': 'styra_diff', 'lnum': lnum,
            \ 'priority': 1,
            \ 'name': line[0] ==# '+' ? 'styraDiffAdded' : 'styraDiffRemoved'})
    endif
  endfor
  call sign_placelist(signs)
endfunction

function! s:is_diff() abort
  return &syntax ==# 'diff' || &syntax ==# 'git'
endfunction

function! s:apply() abort
  if s:is_diff()
    call s:syntax()
    call s:bands()
    if has('nvim')
      lua require('styra_diff').highlight()
    endif
  endif
endfunction

function! s:changed() abort
  if s:is_diff()
    call s:bands()
    if has('nvim')
      lua require('styra_diff').highlight()
    endif
  endif
endfunction

" Another colorscheme takes over: take the bands and rules away again.
function! s:teardown() abort
  augroup styra_diff
    autocmd!
  augroup END
  call sign_unplace('styra_diff')
  silent! sign undefine styraDiffAdded
  silent! sign undefine styraDiffRemoved
  if has('nvim')
    lua require('styra_diff').clear_all()
  endif
  for win in getwininfo()
    call win_execute(win.winid, 'if &syntax =~# ''^\%(diff\|git\)$'' | let &l:syntax = &l:syntax | endif')
  endfor
endfunction

augroup styra_diff
  autocmd!
  autocmd Syntax diff,git call s:apply()
  autocmd TextChanged,TextChangedI,BufWinEnter * call s:changed()
  autocmd ColorSchemePre * call s:teardown()
  if has('nvim')
    autocmd DiffUpdated,WinScrolled,WinResized,BufWinEnter,TextChanged,TextChangedI *
          \ lua require('styra_diff').side_by_side()
    autocmd OptionSet diff lua require('styra_diff').side_by_side()
    " The closing window is still there while this runs.
    autocmd WinClosed * lua vim.schedule(require('styra_diff').side_by_side)
  endif
augroup END

" Buffers already showing a diff.
for s:win in getwininfo()
  call win_execute(s:win.winid, 'call s:apply()')
endfor
unlet! s:win
if has('nvim')
  lua require('styra_diff').side_by_side()
endif

let &cpo = s:cpo
unlet s:cpo
