# mordor-combat

An unofficial Rust / [Bevy](https://bevy.org) reimplementation of the **combat, combat animation, walk / run and crouch / stealth** systems of
*Middle-earth: Shadow of Mordor*, driven by the game's own data: the animation banks, the game database (attack pools, move nodes, counters,
stun and knock-down rules) and the player behaviour graph (gait clips and rates, camera).

**This repository contains no game assets and no character models.** You need your own copy of the game. The program reads the `.arch05`
archives of your install in place at runtime (animations, game database, behaviour graph) and never copies or writes them. Characters are our own
plain humanoid stick figure (a 26-bone rig defined in code, `Skeleton::humanoid`) on a flat baseplate, so no model, skeleton, mesh, texture or material of the game is ever loaded. Climbing / traversal is not part of this project.

## Run

```
set SOM_DIR=C:\Path\To\ShadowOfMordor       (the folder with the .arch05 files; otherwise it is found by walking up from the current directory)
cargo run --release -p som-game
```

An orc stands on the plate and fights back. `cargo run -p som-tool -- <command>` inspects the data (`ls`, `clips <bank>`, `attacks`, `moves`, `nodes`, `applied`, `db ...`; no arguments lists them).

## Controls

| | |
|---|---|
| WASD | move (walk) |
| Shift | run |
| C / Ctrl | crouch (stealth) |
| Space | roll |
| LMB (or L) | attack, tap to chain (free-flow) |
| RMB or Q | counter, when the orc's prompt shows |
| F | execute, when the Hit Streak is charged (every 8 hits) |
| T | provoke the orc, H get hurt |
| Crouch + attack behind an unaware orc | stealth kill |
| Mouse / scroll | look / zoom, Esc releases the mouse |

## What is in it

- `som-formats`: readers for the LTAR archives and BNDL bundles, `.skel`, `.anix` animations (with root motion and cues), the game database, the move nodes and combat tables, the behaviour graph.
- `som-game`: the player (locomotion, free-flow chains, counters, executions, stealth kills, roll, draw / sheathe), orcs with recoils, stuns and attacks, the follow camera, HUD.
- `som-tool`: command line inspection of the data.
- `NOTES.md`: every format and rule decoded, what is approximate, and what is undecoded. `tools/ghidra`: the scripts used to read the executable.

## Legal

Not affiliated with or endorsed by Warner Bros. or Monolith. Shadow of Mordor and its assets belong to their owners. Do not commit or share anything extracted from the game.
Code is MIT licensed (see `LICENSE`).
