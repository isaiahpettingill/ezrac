# Writing Agon MOS Apps In EZRA

This guide covers the common shapes of Agon Light MOS programs written with EZRA: console apps, simple games/visualizations, and graphical apps. Use the `agonlight-mos-ez80` target for all examples here.

```toml
[build]
target = "agonlight-mos-ez80"
output = "bin"
```

Build with:

```sh
ezrac build src/main.ezra
```

From an `ezrac` checkout, use:

```sh
cargo run -- build src/main.ezra
```

The output is a normal Agon MOS executable. Let `main` return when the app is done; emulator-only exit ports are for automation, not user-facing MOS programs.

## Pick The Right SDK Modules

Use these modules as your starting point:

| App type | Modules | Use for |
| --- | --- | --- |
| Console app | `agon.console`, `agon.keyboard` | menus, prompts, blocking key reads, text UI |
| Visualization | `agon.console`, `agon.vdp`, `agon.keyboard` | immediate-mode shapes, charts, demos, simple loops |
| Game | `agon.console`, `agon.keyboard`, `agon.vdp`, `agon.sprites` | hardware sprites, HUD text, input polling, frame pacing |
| Graphical app | `agon.console`, `agon.vdp`, optionally `agon.mouse` | mode setup, palettes, viewports, drawing, pointer input |

Import only the modules you use:

```ezra
import agon.console
import agon.keyboard
import agon.vdp
```

## Console Apps

Console apps are the easiest Agon programs to write. Use `console.clear()`, `console.print()`, `console.print_line()`, `console.tab()`, and `console.read_key()` for menus and prompts.

```ezra
import agon.console

fn main() {
    console.clear()
    console.color(7)
    console.print_line("EZRA CAFE")
    console.print_line("")
    console.print_line("1) Coffee")
    console.print_line("2) Latte")
    console.print("Pick 1-2: ")

    console.clear_keyboard_state()
    let choice: u8 = console.read_key()
    console.write(choice)
    console.newline()

    if choice == '1' {
        console.print_line("Coffee ready")
        return
    }

    if choice == '2' {
        console.print_line("Latte ready")
        return
    }

    console.print_line("No order")
}
```

Console guidelines:

- Use `console.print_line()` for CR/LF line endings.
- Use `console.tab(x, y)` for simple text layout.
- Call `console.clear_keyboard_state()` before a single blocking key read if prior input should be ignored.
- Prefer returning from `main` over calling emulator exit helpers.

## Games And Visualizations

For simple animation or visualization, use VDP mode 8 and immediate-mode drawing. The SDK exposes mode-8 constants so you do not need to repeat common dimensions.

```ezra
import agon.console
import agon.keyboard
import agon.vdp

global x: u16 = 16
global dx: u16 = 4

fn draw() {
    vdp.clear_graphics_color(vdp.COLOR_BLACK)
    vdp.draw_color(vdp.COLOR_CYAN)
    vdp.filled_rectangle(x, 92, x + 31, 123)
    vdp.draw_color(vdp.COLOR_BRIGHT_WHITE)
    vdp.rectangle(x, 92, x + 31, 123)
}

fn step() {
    if dx == 4 && x >= vdp.MODE_8_RIGHT - 32 {
        dx = 0
    }

    if dx == 0 && x <= 4 {
        dx = 4
    }

    if dx == 4 {
        x += 4
    } else {
        x -= 4
    }
}

fn main() {
    console.clear()
    console.cursor_enable(0)
    vdp.mode_8()
    keyboard.clear_state()

    loop {
        if keyboard.ascii() == 'q' || keyboard.ascii() == 'Q' {
            return
        }

        step()
        draw()
        keyboard.clear_state()
        vdp.frame_delay()
    }
}
```

Loop guidelines:

- Keep input, update, draw, and delay as separate functions.
- Use `keyboard.ascii()` for polled input and `keyboard.clear_state()` after consuming it.
- Use `vdp.frame_delay()` for simple demos; replace it with VDP flag waits or timer-based pacing when exact timing matters.
- Use immediate drawing for low object counts and one-off visualizations.

## Sprite-Based Games

Use `agon.sprites` when objects move every frame. A typical sprite game does this:

1. Set screen mode and coordinate mode.
2. Draw small bitmap art into off-screen screen areas.
3. Capture each bitmap with `sprites.capture_bitmap()`.
4. Assign bitmaps to sprites.
5. In the loop, update positions, move sprites, call `sprites.update()`, and delay.

Skeleton:

```ezra
import agon.console
import agon.keyboard
import agon.sprites
import agon.vdp

const PLAYER_BITMAP: u8 = 0
const PLAYER_SPRITE: u8 = 0

global player_x: u16 = 144

fn draw_player_bitmap() {
    vdp.draw_color(vdp.COLOR_GREEN)
    vdp.triangle(8, 32, 24, 32, 16, 16)
    vdp.move_to(8, 16)
    vdp.move_to(24, 32)
    sprites.capture_bitmap(PLAYER_BITMAP)
}

fn setup_player_sprite() {
    sprites.select_sprite(PLAYER_SPRITE)
    sprites.clear_frames()
    sprites.add_frame(PLAYER_BITMAP)
    sprites.select_frame(0)
    sprites.set_hardware()
    sprites.show()
}

fn sync_player() {
    sprites.select_sprite(PLAYER_SPRITE)
    sprites.move_sprite_to(player_x, 180)
    sprites.show()
    sprites.update()
}

fn handle_input() {
    let key: u8 = keyboard.ascii()
    if key == 'a' || key == 'A' {
        if player_x > 4 {
            player_x -= 4
        }
    }
    if key == 'd' || key == 'D' {
        if player_x < vdp.MODE_8_RIGHT - 24 {
            player_x += 4
        }
    }
    keyboard.clear_state()
}

fn main() {
    console.clear()
    console.cursor_enable(0)
    vdp.mode_8()
    sprites.enable_hardware_sprites()
    draw_player_bitmap()
    console.clear()
    setup_player_sprite()

    loop {
        if keyboard.ascii() == 'q' || keyboard.ascii() == 'Q' {
            sprites.reset_sprites()
            return
        }
        handle_input()
        sync_player()
        vdp.frame_delay()
    }
}
```

Sprite guidelines:

- Build or load all sprite frames before the main loop.
- Keep game state in globals or small helper functions until EZRA has richer aggregate ergonomics.
- Always hide or reset sprites before returning to MOS.
- Use `examples/agon-mos/space-invaders` as the larger reference.

## Graphical Apps

For non-game graphical apps, use immediate-mode VDP drawing plus text labels. Viewports and origins help isolate drawing regions.

```ezra
import agon.console
import agon.vdp

fn draw_panel() {
    vdp.draw_color(vdp.COLOR_BLUE)
    vdp.filled_rectangle(20, 40, 300, 210)
    vdp.draw_color(vdp.COLOR_BRIGHT_WHITE)
    vdp.rectangle(20, 40, 300, 210)
    vdp.draw_color(vdp.COLOR_YELLOW)
    vdp.line(40, 180, 120, 80)
    vdp.line(120, 80, 200, 140)
    vdp.line(200, 140, 280, 60)
}

fn main() {
    console.clear()
    console.cursor_enable(0)
    vdp.mode_8()
    draw_panel()
    console.tab(2, 1)
    console.color(vdp.COLOR_BRIGHT_WHITE)
    console.print_line("EZRA GRAPH")
}
```

Graphics guidelines:

- Call `vdp.mode_8()` once at startup for a known 320x240 logical drawing area.
- Use `vdp.draw_color(color)` before shape calls.
- Use `vdp.graphics_viewport()` and `vdp.reset_viewport()` for clipped panels.
- Use `vdp.origin()` when local coordinates make code easier to read.
- Use `agon.mouse` when the app needs pointer input.

## Build And Run

List documented targets:

```sh
ezrac targets
```

Build an Agon app:

```sh
ezrac build --target agonlight-mos-ez80 src/main.ezra
```

Run with Fab Agon Emulator if you have it installed locally:

```powershell
$env:FAB_AGON_EMULATOR_DIR = "K:\source\fab-agon-emulator"
pwsh tools/run-fab-agon.ps1 path\to\program.bin
```

See also:

- `examples/agon-mos/console` for console basics.
- `examples/agon-mos/sdk-showcase` for SDK coverage.
- `examples/agon-mos/space-invaders` for a sprite game.
- `docs/platforms.md` for target profiles and memory layout notes.
