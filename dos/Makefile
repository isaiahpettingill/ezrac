RUST_CONFIG ?= release
CARGO_FLAGS := --release
ifeq ($(RUST_CONFIG),debug)
CARGO_FLAGS :=
endif

TARGET_DIR := target/dos/$(RUST_CONFIG)
RUST_LIB := $(TARGET_DIR)/libezrac_dos.a
WCC386 := $(WATCOM)/binnt64/wcc386.exe
WLINK := $(WATCOM)/binnt64/wlink.exe
DOS32A := $(WATCOM)/binw/dos32a.exe
WLINK_CONFIG := $(WATCOM)/binnt64/wlink.lnk

.PHONY: all check-tools clean run

all: check-tools stack.obj entry.obj
	cargo build $(CARGO_FLAGS) --manifest-path Cargo.toml --target dos.json -Zjson-target-spec -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem
	mkdir -p $(TARGET_DIR)
	"$(WLINK)" @"$(WLINK_CONFIG)" name $(TARGET_DIR)/ezrac system dos32x option dosseg option stack=4194304 option stub="$(DOS32A)" file entry.obj file stack.obj library $(RUST_LIB)

check-tools:
	@test -n "$(WATCOM)" || (echo "error: set WATCOM to the OpenWatcom install directory" >&2; exit 1)
	@test -x "$(WCC386)" || (echo "error: OpenWatcom compiler not found at $(WCC386)" >&2; exit 1)
	@test -x "$(WLINK)" || (echo "error: OpenWatcom linker not found at $(WLINK)" >&2; exit 1)

stack.obj: stack.nasm
	nasm -f obj stack.nasm -o stack.obj

entry.obj: entry.c
	"$(WCC386)" -4s -bt=dos -fo=entry.obj entry.c

run: all
	dosbox $(TARGET_DIR)/ezrac.exe

clean:
	cargo clean --manifest-path Cargo.toml
	rm -f stack.obj entry.obj
