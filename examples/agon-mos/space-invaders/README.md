# Agon MOS Space Invaders

A small Space Invaders-style game written in EZRA for the Agon Light MOS target.

## Build

```sh
cargo run -- build examples/agon-mos/space-invaders/src/main.ezra
```

The executable is written to `target/agonlight-mos-ez80/src/space-invaders.bin` under this example directory.

## Controls

- `A` or `Z`: move left
- `D` or `X`: move right
- `Space`: fire
- `Q`: quit to MOS

Destroy all invaders before the fleet reaches the player.
