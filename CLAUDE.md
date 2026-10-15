# Wrath-Rust (pf-wotr-rs)

A Rust/Bevy engine for Pathfinder: Wrath of the Righteous, built for Ember's Apple Silicon
Mac. It loads the game's data from Ember's install. **Read `PLAN.md` before doing
anything**: it's the handoff, with the goal, the fidelity contract, decisions, facts
measured so far, and the proof-of-concept milestones. `docs/research/` holds the code
survey and notes on other rewrites.

- **Current status (2026-10-15, twenty-first session; PLAN.md's "Where things stand" section is the handoff, read it first):** POC-0 and **POC-1 (oracle
  certification) passed**. Ember's fixture saves exist: `Manual_18_WR_FirstBattle`,
  `Manual_19_WR_PreHorgus` and `Manual_20_WR_PreCamelia` (read-only copies in `fixtures/saves/`, gitignored).
  Where each workstream stands, with its handoff:
  - POC-4 (`docs/poc4/dialogue.md`, round 12; text: `docs/poc4/text.md`): every dialogue run is valid end to end,
    its text included (`crates/wr-text`), on `poc4-seelah-7` (12 runs), `poc4-horgus-11` (6), `cert-24` (3) and
    `poc4-camelia-6` (4: Camelia's cutscene-started dialogue and recruitment, from Ember's `Manual_20_WR_PreCamelia`;
    `poc4-camelia-5` is a development capture with a teleport). Facts' handler interfaces: done (round 13). Next: PLAN's list.
  - POC-5 (`docs/poc5/combat.md`, `docs/poc5/commands.md` round 14 slice G): every run of `poc5-tb-95` and `-96` valid (three fights with different choices: the pass as worded)
    with cooldowns, turns, the commands' lifecycle, `UnitAttack`, the approach, the AI, the engagement controller,
    units' `LastTarget`/`ManualTarget` and the impatience controller computed (`combat/commands/*.rs`, `combat/ai.rs`,
    `combat/engage.rs`, `combat/targets.rs`, `combat/impatience.rs`, `combat/replay/*.rs`; oracle `CommandObs*.cs`,
    observers `commands`, `attacks`, `approach`, `ai`, `engage`, `targets`, `impatience`). Next: see PLAN's list; the
    movement agent waits for the nav port.
  - Oracle (`docs/poc5/tb-replay.md`): turn-based replays are tick-exact, under load too (`poc5-tb-51`..`-53`,
    player input neutral at the source, physics phase pinned); `cert-20` certifies from this repo's layout.
  - POC-6 render round 3 (`docs/poc6/render-round3.md`): realtime shadows; FLIP 0.1246. Next: light probes.
  - **booklab** (`ssh booklab`, an M1, dedicated): **not available for now (Ember, 2026-10-13): plan for this Mac
    only.** When it's back: a second Mac that runs the game for correctness work in
    parallel (recordings, POC-6 render comparisons, Metal captures of the original), never timing. Setup, sync and
    use: `docs/booklab.md`. Certified for fights; dialogue plans run on this Mac (a post-dialogue reload stalls there).
  - Record with `scripts/oracle-record.sh`. For unattended runs keep `caffeinate -dim` going: the
    display sleeps after 120 min and locks the session, and game runs hang while it's locked.

  **The oracle lives in this repo** (`oracle/`, see `oracle/README.md`) and runs on
  `builds/arm64-oracle` (gitignored APFS clone; no ToyBox or BuffIt2TheLimit). Ember wants both
  mods recreated in our version later (PLAN, After the POC, item 4). Implementation subagents run on **Sonnet** (Opus only
  when Sonnet falls short), at most 3 at once, and every brief says "don't spawn
  sub-agents". Game and GPU share one lock, `builds/.oracle-run.lock`; match
  the game with `pgrep -x 'Pathfinder Wrath Of The Righteous'`, never `-f`.
- **Toolchain:** Rust via Homebrew `rustup` (keg-only; PATH set in `~/.zshenv`; a shell
  started before that may need `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`).
  `scripts/check.sh` checks the main workspace; `scripts/check.sh spikes/<name>` a spike.
  Disk (2026-10-14): `target/`, `cache/` and `fixtures/captures/` are symlinks onto Ember's external SSD volume (2.5 TB free; the location is in Claude's memory), so they can grow; the internal disk has ~94 GiB free and keeps the repo, `builds/` (the oracle build is an APFS clone, which only shares blocks on its own volume, and timing runs stay internal), `decompiled/` and the saves. Builds and recordings need the volume mounted. Superseded captures go to an archive folder on that volume (Ember deletes them). A POC-5 worktree has its own `target/` (internal unless linked too). A 5-minute Metal System Trace is ~22 GB, so keep traces short.
- **Sister project, now separate:** `~/Developer/Wrath-Mac` is **pf-wotr-silicon** (the
  native arm64 build tooling and WrathPerf, made public-ready 2026-10-12). Since then
  **pf-wotr-rs doesn't use or touch it**: everything this project needs lives here. Game-side
  tooling (WrathOracle, WrathStats, WrathBench and the oracle tools) is under `oracle/`. The
  decompiled game code is in `decompiled/`, which is gitignored: never commit it or copy it
  into tracked files.
- **Git workflow (Ember, 2026-10-11):** commit on a work branch (currently `poc`), never directly on `main`; then fast-forward `main` to it
  (`git fetch . poc:main`, which leaves the working tree alone). Ember has handed the
  fast-forwarding to Claude. Don't push unless asked.
- **Never commit game data, extracted assets, decompiled code or oracle captures.** Keep
  them under the gitignored paths and check `git status` before every commit. The remote
  (`loafykitten/pf-wotr-rs`) is public (since 2026-10-14) with a fresh history (the old one is on the
  local, never-pushed branch `archive/private-history`): keep tracked files free of machine-specific paths
  (scripts find the repo from their own location) and of personal details.
- **Model routing:** Claude implements (here, or through **Sonnet** subagents; Ember,
  2026-10-11: escalate to Opus only when Sonnet's work is bad or it can't solve the task). **gpt-6.1-sol (high)** reviews every implementation (`codex-review` skill, Codex Fast mode); **GPT 6 Astra (high)** instead when Sol misses things or the work is particularly complex or touchy (Ember, 2026-10-12: Astra is slow even in Fast mode). Use
  **gpt-6.1-sol**, not gpt-5.6-sol, for Codex bulk investigation and computer use
  (`codex exec -m gpt-6.1-sol`); Ember said 2026-10-10 that it supersedes 5.6, and it
  overrides the global CLAUDE.md and any skill that names 5.6.
- **Ember's standing decisions** (details in PLAN.md):
  - rules, story state and saves match the original exactly;
  - everything else is free to redesign but must be measurably better;
  - the picture must not change perceivably. Upscaling (MetalFX) and similar techniques
    are fine as options, and every option says whether it's original or an addition;
  - read original saves, but write only our own, faster format;
  - test turn-based combat first;
  - Ember makes fixture saves by playing;
  - licensing is settled: continue, and don't reopen it.
