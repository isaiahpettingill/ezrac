# Target and layout internals

A target profile connects a CPU/backend to an address model, default layout, startup/runtime assembly, SDK modules, and output package. A resolvable CPU profile does not imply complete source or runtime support.

## Target model

Target selection determines:

- CPU syntax and instruction encoder;
- address and pointer width;
- scalar ABI and function result locations;
- default memory regions and output sections;
- startup, entry, and stack behavior;
- bundled SDK family;
- executable packaging and extension.

The target registry also provides the information printed by `ezrac targets`. Keep its documented support status aligned with tests and the relevant target guide.

## Layout model

A layout contains:

- `load`, `entry`, and `stack` addresses;
- named memory regions with access flags;
- logical sections mapped to regions and alignments;
- symbols consumed by startup/runtime code or exported to the map.

The linker places generated code, data, strings, and embeds through this model. It validates address width, region permissions, alignment, section overflow, and the final assembled text size before packaging.

## Support evidence

The platform guide uses four tiers:

- **Tier 1** — representative examples verified on a real third-party core.
- **Tier 2** — compiler, VM/emulator, SDK, and packaging tests cover source behavior.
- **Tier 3** — build, assembly, or packaging paths are tested; external runtime validation is not published.
- **Tier 4** — profile or SDK scaffolding exists, but major backend/runtime validation remains.

A target can move between tiers as evidence changes. Do not turn a Tier 3 or Tier 4 profile into a portability promise in SDK or language documentation.
