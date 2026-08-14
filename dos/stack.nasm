segment code public use32 class=code
global _dos_open
global __alloca

_dos_open:
    mov edx, [esp + 4]
    mov eax, [esp + 8]
    mov ah, 3dh
    push ebx
    push esi
    push edi
    push ebp
    int 21h
    pop ebp
    pop edi
    pop esi
    pop ebx
    jc .error
    movzx eax, ax
    ret
.error:
    movzx eax, ax
    or eax, 80000000h
    ret

__alloca:
    push ecx
    cmp eax, 1000h
    lea ecx, [esp + 8]
    jb .last
.probe:
    sub ecx, 1000h
    test dword [ecx], ecx
    sub eax, 1000h
    cmp eax, 1000h
    ja .probe
.last:
    sub ecx, eax
    test dword [ecx], ecx
    lea eax, [esp + 4]
    mov esp, ecx
    mov ecx, [eax - 4]
    push dword [eax]
    sub eax, esp
    ret
