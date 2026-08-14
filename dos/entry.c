extern void rust_start(void);
#pragma aux rust_start "_rust_start";

int main(void) {
    rust_start();
    return 0;
}
