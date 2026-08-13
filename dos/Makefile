RUST_CONFIG ?= release
CARGO_FLAGS := --release
ifeq ($(RUST_CONFIG),debug)
CARGO_FLAGS :=
endif

TARGET_DIR := target/dos/$(RUST_CONFIG)
RUST_LIB := $(TARGET_DIR)/libezrac_dos.a

.PHONY: all clean run

all: stack.obj
	cargo build $(CARGO_FLAGS) --manifest-path Cargo.toml --target dos.json -Zjson-target-spec -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem
	mkdir -p $(TARGET_DIR)
	PATH="$(WATCOM)/binl:$(WATCOM)/binw:$$PATH" wlink name $(TARGET_DIR)/ezrac system dos32x option dosseg option start=_start file stack.obj library $(RUST_LIB)

stack.obj: stack.nasm
	nasm -f obj stack.nasm -o stack.obj

run: all
	dosbox $(TARGET_DIR)/ezrac.exe

clean:
	cargo clean --manifest-path Cargo.toml
	rm -f stack.obj
