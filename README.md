# pf-wotr-rs

A new engine for **Pathfinder: Wrath of the Righteous**, written in Rust, built to run the
game on Apple Silicon Macs better than the original does.

It isn't a remake and it doesn't ship any of the game. It reads the game's own data (its rules,
dialogue, maps, art and saves) from an installed copy you already own, and runs it on new code.

## Why

Wrath is a wonderful game, but its Mac version is built for Intel Macs, so on Apple Silicon it
doesn't run as well as it could.

The rewrite runs the same game, with the same rules, the same outcomes and the same story, on
code built for this hardware.

It's also about keeping the game playable. On Apple Silicon, Intel apps run through Rosetta,
Apple's translation layer, and
[according to Apple](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment)
Rosetta stays available as a general-purpose tool only through macOS 27. After that, Apple
plans to keep just a subset of it, aimed at older games that are no longer maintained, with no
guarantee about any particular game. An engine built for Apple Silicon doesn't need Rosetta at
all, so the game keeps running on future versions of macOS.

The bar for the rewrite being worth using is simple: it has to perform better than the
original.

## The rules

- **The game must play exactly the same.** Every dice roll, every rule, every story choice and
  every save comes out the same as the original. This isn't approximate: it's checked.
- **Everything else is free to improve,** as long as the improvement can be measured.
- **The picture must not change in a way you'd notice.** Optional extras (like Apple's MetalFX
  upscaling) are welcome, and every option says whether it's the original's or an addition.
- **Your saves keep working.** The new engine reads the original's saves; it writes its own,
  faster format.

## How we know it's the same game

The original game is the referee. A small helper mod, the **oracle**, runs inside the real game
and records everything that happens: each dice roll, each rule the game applies, each decision
the enemies make. The new engine then replays the same moments and must arrive at exactly the
same results, step by step. If it ever disagrees, or meets something it hasn't learned yet, it
says so instead of guessing.

Over time, more and more of the game's behaviour moves from "taken from the recording" to
"worked out by the new engine and checked against the recording." When nothing is taken from
the recording any more, that part of the game is done.

## Where things stand

The first goal is a **proof of concept**: one small part of the game (the caves beneath
Kenabres at the start of the story, with their first fight and the meetings with Camelia,
Seelah and Anevia), played from a real save. It ends in a go or no-go decision backed by
measurements: is the full rewrite doable, and what would the rest cost?

| Step | What it proves | Status |
|---|---|---|
| 0. Setup | The risky unknowns are understood before building on them | Done |
| 1. The referee | The oracle records the real game faithfully and repeatably | Done |
| 2. Game data | All of the game's data files can be read correctly | Mostly done; the final check against the live game is still to come |
| 3. Saves | Original saves load with every character sheet matching the game's | Well along: the caves save matches the game's sheets exactly; a later, bigger save matches about 80% so far |
| 4. Dialogue | Conversations, skill checks and their consequences play out identically | Most conversations match end to end; one still depends on a cutscene detail not yet built |
| 5. A fight | A whole turn-based fight plays out identically, start to finish | Well along: recorded fights replay exactly, and dice, turns, attacks, the enemies' decisions and attacks of opportunity are now worked out by the new engine; movement and a few other inputs still come from the recording |
| 6. Graphics | The caves render like the original, faster, on Apple Silicon | Underway: the caves render with the game's own shaders, lighting, fog, bloom and shadows; characters, effects and the rest of the image are still to come |
| 7. All together | The slice is playable start to finish in the new engine | Not started |
| 8. The cost | An honest estimate of what the whole game would take | Running alongside the others |

Under the hood, every piece of the work is reviewed independently before it lands, and every
check is built to fail loudly rather than pass by accident.

## Where we're going

After the proof of concept, and only if it says go:

1. **The prologue, playable,** from Kenabres to the end of the caves, every fight and cutscene.
2. **Character creation and levelling up.**
3. **Act 1 and beyond,** guided by a tracker of which parts of the game the engine supports.
   The world map, the kingdom and the crusade are projects of their own.
4. **Mods.** Mods written for the original can't run on a new engine, so by the end
   **ToyBox** and **Buff It 2 The Limit** will be ported over as mods for this one. They stay
   separate and optional, not built into the engine, and the engine itself stays the original
   game.

## What's in this repository

- `crates/`: the engine. Reading the game's data and saves (`wr-content`, `wr-save`,
  `wr-assets`), the game's rules and simulation (`wr-sim`), and the graphics (`wr-game`).
- `oracle/`: the referee mods and tools that run inside the original game.
- `docs/`: the notes for each step, with everything measured so far.
- `PLAN.md`: the full plan, the decisions made, and where work stands right now.

You need your own copy of Wrath of the Righteous to use any of it. This repository contains no
game files, extracted assets or game code.

## Credits

This project stands on other people's work:

- **[Owlcat Games](https://owlcatgames.com)** made Pathfinder: Wrath of the Righteous. Its
  rules, story, art and data are theirs; this engine only reads them from your own copy.
- **[Paizo](https://paizo.com)** created Pathfinder and the Wrath of the Righteous adventure
  path the game is based on.
- **[Unity](https://unity.com)** and the **[Mono](https://www.mono-project.com)** project:
  the new engine reproduces how the original's runtime behaves, down to its arithmetic and
  collections, and ports a few pieces of Mono's and .NET's code (see
  `THIRD_PARTY_NOTICES.md`).
- **[Bevy](https://bevyengine.org)** and the Rust crates in `Cargo.lock` power the engine.
- **[Harmony](https://github.com/pardeike/Harmony)** by Andreas Pardeike and
  **[Unity Mod Manager](https://github.com/newman55/unity-mod-manager)** by newman55 make the
  referee mods possible.
- **[ILSpy](https://github.com/icsharpcode/ILSpy)** reads the original's code, and
  **[NVIDIA FLIP](https://github.com/NVlabs/flip)** measures how close our pictures are to
  the original's.
- **[ToyBox](https://github.com/cabarius/ToyBox)** by cabarius and its contributors, and
  **[Buff It 2 The Limit](https://github.com/Gh05d/wrath-epic-buffing)** by Gh05d, continuing
  factubsio's **[BubbleBuffs](https://github.com/factubsio/BubbleBuffs)**, are the mods we
  plan to port.

This is a fan project. It isn't affiliated with or endorsed by Owlcat Games or Paizo.
Pathfinder is a trademark of Paizo Inc.

## License

This project's own code is under the [MIT License](LICENSE). That covers only the code written
here: the game, its data and everything named above stay their owners', under their own terms.
Parts ported from Mono and .NET keep their notices in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
