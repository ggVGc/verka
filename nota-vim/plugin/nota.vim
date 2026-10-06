if exists('g:loaded_nota')
  finish
endif
let g:loaded_nota = 1

command! -nargs=* NotaStart call nota#command('start', [<f-args>])
command! -nargs=? NotaShow call nota#command('show', [<f-args>])
command! -nargs=? NotaBranch call nota#command('branch', [<f-args>])
command! -nargs=* NotaSuggest call nota#command('suggest', [<q-args>])
command! -range -nargs=* NotaNote call nota#command('note', [<q-args>, <range>, <line1>, <line2>])
