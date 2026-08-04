if exists('b:current_syntax')
  finish
endif

syn keyword ezraKeyword import const alias port mmio embed global struct extern asm fn layout load entry stack region section symbol let out in cast
syn keyword ezraControl if else while loop break continue return
syn keyword ezraModifier pub inline naked interrupt volatile as clobber align read write execute reserved
syn keyword ezraType file bytes text cstr repeat ptr u8 i8 u16 i16 u24 i24
syn keyword ezraBoolean true false

syn match ezraNumber "\v\<0x[0-9A-Fa-f]+(u8|i8|u16|i16|u24|i24)?\>"
syn match ezraNumber "\v\<0b[01]+(u8|i8|u16|i16|u24|i24)?\>"
syn match ezraNumber "\v\<[0-9]+(u8|i8|u16|i16|u24|i24)?\>"
syn match ezraOperator "->\|\.\.\|<<=\|>>=\|==\|!=\|<=\|>=\|&&\|||\|<<\|>>\|[-+*/%&|^~!=<>]=?"
syn match ezraComment "//.*$"
syn match ezraChar "'\(\\[n0t\\'\"]\|[^'\\]\)'"
syn region ezraString start='"' skip='\\.' end='"' contains=ezraEscape
syn match ezraEscape "\\[n0t\\'\"]" contained
syn match ezraFunction "\v\<fn\s+\zs[A-Za-z_][A-Za-z0-9_]*"

hi def link ezraKeyword Keyword
hi def link ezraControl Conditional
hi def link ezraModifier StorageClass
hi def link ezraType Type
hi def link ezraBoolean Boolean
hi def link ezraNumber Number
hi def link ezraOperator Operator
hi def link ezraComment Comment
hi def link ezraString String
hi def link ezraChar Character
hi def link ezraEscape SpecialChar
hi def link ezraFunction Function

let b:current_syntax = 'ezra'
