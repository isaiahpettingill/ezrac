# Control flow and expressions

## Local bindings and assignment

Local variables require a type and initializer:

```ezra
let count: u8 = 0
count = 1
count += 1
count <<= 1
```

Compound assignment supports `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, and `>>=`.

## Conditions and loops

```ezra
if value == 0 {
    return
} else if value < 10 {
    value += 1
} else {
    value = 0
}

while value < 10 {
    value += 1
}

loop {
    if value == 0 { break }
    value -= 1
}
```

`break` and `continue` apply to loops. `return` can be empty, return one expression, or return two expressions matching the function signature. There is no general `for` syntax; use `while` or `loop`.

## Expressions

Supported forms include:

```ezra
name
module.name
function(arg1, arg2)
array[index]
object.field
&name
&array[index]
*object_pointer
cast<u16>(value)
in PORT
```

Array indexing, field access, address-of, dereference, calls, and casts are checked against the declared types. Use parentheses to make mixed arithmetic and bitwise expressions clear.

## Defined arithmetic

Integer operations wrap at the declared width. Signed division truncates toward zero, the remainder has the dividend's sign, and division or remainder by zero produces zero. Use `ezra.int.saturating_add` and `saturating_sub` when wrapping is not wanted. Target support for a legal source expression can still vary.
