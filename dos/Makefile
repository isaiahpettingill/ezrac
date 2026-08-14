RUST_CONFIG ?= release
CARGO_FLAGS := --release
ifeq ($(RUST_CONFIG),debug)
CARGO_FLAGS :=
endif

TARGET_NAME := dos
OUTPUT_DIR := target/dos/$(RUST_CONFIG)
OBJ_DIR := target/openwatcom/$(RUST_CONFIG)
WCC386 := $(WATCOM)/binnt64/wcc386.exe
WLINK := $(WATCOM)/binnt64/wlink.exe
DOS32A := $(WATCOM)/binw/dos32a.exe
WLINK_CONFIG := $(WATCOM)/binnt64/wlink.lnk
PYTHON ?= python
MKISOFS ?= mkisofs
STACK_OBJ := $(OBJ_DIR)/stack.obj
ENTRY_OBJ := $(OBJ_DIR)/entry.obj
CARGO_BUILD := cargo build $(CARGO_FLAGS) --manifest-path Cargo.toml --target dos.json -Zjson-target-spec -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem --no-default-features

.PHONY: all check-tools rust-libs test-floppies test-cd clean

all: check-tools rust-libs $(OUTPUT_DIR)/ezfe.exe $(OUTPUT_DIR)/ezopt.exe $(OUTPUT_DIR)/ezcg.exe $(OUTPUT_DIR)/ezas.exe
	cp EZRAC.BAT $(OUTPUT_DIR)/EZRAC.BAT

test-floppies: all
	$(PYTHON) make_test_floppies.py

test-cd: all
	rm -rf target/freedos-cd/root
	mkdir -p target/freedos-cd/root
	cp $(OUTPUT_DIR)/ezfe.exe target/freedos-cd/root/EZFE.EXE
	cp $(OUTPUT_DIR)/ezopt.exe target/freedos-cd/root/EZOPT.EXE
	cp $(OUTPUT_DIR)/ezcg.exe target/freedos-cd/root/EZCG.EXE
	cp $(OUTPUT_DIR)/ezas.exe target/freedos-cd/root/EZAS.EXE
	cp $(OUTPUT_DIR)/EZRAC.BAT target/freedos-cd/root/EZRAC.BAT
	cp ../examples/msdos-i8086/hello/hello.ezra target/freedos-cd/root/HELLO.EZR
	cp INSTALL.CD.BAT target/freedos-cd/root/INSTALL.BAT
	$(MKISOFS) -quiet -iso-level 1 -V EZRACDOS -o target/freedos-cd/ezrac-freedos.iso target/freedos-cd/root

check-tools:
	@test -n "$(WATCOM)" || (echo "error: set WATCOM to the OpenWatcom install directory" >&2; exit 1)
	@test -x "$(WCC386)" || (echo "error: OpenWatcom compiler not found at $(WCC386)" >&2; exit 1)
	@test -x "$(WLINK)" || (echo "error: OpenWatcom linker not found at $(WLINK)" >&2; exit 1)
	@test -x "$(DOS32A)" || (echo "error: DOS/32A stub not found at $(DOS32A)" >&2; exit 1)
	@test -f "$(WLINK_CONFIG)" || (echo "error: OpenWatcom linker config not found at $(WLINK_CONFIG)" >&2; exit 1)

rust-libs:
	CARGO_TARGET_DIR=target/ezfe $(CARGO_BUILD) --features ezfe
	CARGO_TARGET_DIR=target/ezopt $(CARGO_BUILD) --features ezopt
	CARGO_TARGET_DIR=target/ezcg $(CARGO_BUILD) --features ezcg
	CARGO_TARGET_DIR=target/ezas $(CARGO_BUILD) --features ezas

$(OBJ_DIR):
	mkdir -p $(OBJ_DIR)

$(OUTPUT_DIR):
	mkdir -p $(OUTPUT_DIR)

$(STACK_OBJ): stack.nasm | $(OBJ_DIR)
	nasm -f obj stack.nasm -o $(STACK_OBJ)

$(ENTRY_OBJ): entry.c | $(OBJ_DIR)
	"$(WCC386)" -4s -bt=dos -fo=$(ENTRY_OBJ) entry.c

$(OUTPUT_DIR)/ezfe.exe: rust-libs $(ENTRY_OBJ) $(STACK_OBJ) | $(OUTPUT_DIR)
	"$(WLINK)" @"$(WLINK_CONFIG)" name $@ system dos32x option dosseg option stack=4194304 option stub="$(DOS32A)" file $(ENTRY_OBJ) file $(STACK_OBJ) library target/ezfe/$(TARGET_NAME)/$(RUST_CONFIG)/libezrac_dos.a

$(OUTPUT_DIR)/ezopt.exe: rust-libs $(ENTRY_OBJ) $(STACK_OBJ) | $(OUTPUT_DIR)
	"$(WLINK)" @"$(WLINK_CONFIG)" name $@ system dos32x option dosseg option stack=4194304 option stub="$(DOS32A)" file $(ENTRY_OBJ) file $(STACK_OBJ) library target/ezopt/$(TARGET_NAME)/$(RUST_CONFIG)/libezrac_dos.a

$(OUTPUT_DIR)/ezcg.exe: rust-libs $(ENTRY_OBJ) $(STACK_OBJ) | $(OUTPUT_DIR)
	"$(WLINK)" @"$(WLINK_CONFIG)" name $@ system dos32x option dosseg option stack=4194304 option stub="$(DOS32A)" file $(ENTRY_OBJ) file $(STACK_OBJ) library target/ezcg/$(TARGET_NAME)/$(RUST_CONFIG)/libezrac_dos.a

$(OUTPUT_DIR)/ezas.exe: rust-libs $(ENTRY_OBJ) $(STACK_OBJ) | $(OUTPUT_DIR)
	"$(WLINK)" @"$(WLINK_CONFIG)" name $@ system dos32x option dosseg option stack=4194304 option stub="$(DOS32A)" file $(ENTRY_OBJ) file $(STACK_OBJ) library target/ezas/$(TARGET_NAME)/$(RUST_CONFIG)/libezrac_dos.a

clean:
	cargo clean --manifest-path Cargo.toml
	rm -rf target/ezfe target/ezopt target/ezcg target/ezas target/openwatcom
