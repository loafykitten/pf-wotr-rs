# POC-4: text — handoff

Written 2026-10-14 (nineteenth session). Tags: **[M]** measured, **[C]** read in the
decompiled C#, **[I]** inferred. No game text here: keys, GUIDs, type names and counts only.
POC-4 asks for "text with template expansion"; this round ports it and checks it against the
game's own output in every dialogue run.

## Headline

| | Result |
|---|---|
| `crates/wr-text` | The localization packs and `TextTemplateEngine` ported. Our pack format opens in 1.9 ms where the game's JSON takes 66 ms to read; all 110,399 English keys looked up and equal (135 ns each) [M] |
| The template scan against the C#'s | **Equal on 335,690 strings [M]**: every string with a `{` in four packs (enGB, ruRU, zhCN, deDE) and 200,000 fuzzed ones, the decompiled `Process` loop run on Mono (the game's runtime) beside ours, every registered template replaced by the same stand-in; 47,414 of them throw in both |
| Dialogue text against the game | **Equal on all 21 dialogue runs [M]** (`poc4-seelah-7`'s 12, `poc4-horgus-11`'s 6, `cert-24`'s 3): every history line the game raises, every cue's and shown answer's display text, every speaker's name and colour, in the ordered T0 stream; every run stays valid |

## The game's text [C]

- **Packs** (`Kingmaker.Localization/LocalizationPack.cs`, `LocalizationManager.cs`): the build
  reads `StreamingAssets/Localization/<locale>.json` (`UsePackedLocalization` is off), one
  `{"strings": {key: text}}` object into a `Dictionary<string, StringEntry>`. `GetText(key)`
  returns the text, `""` for a null one, and `[unknown key: …]` for a missing key (or `""`
  when not reporting). `Sound.json` maps keys to voice-over events.
- **`LocalizedString`** (`LocalizedString.cs`): the implicit conversion to `string` looks up
  the actual key (`m_Key`, or the end of the `Shared` chain, at most 50 steps), `""` for an
  empty key, `"<null>"` for a null string, and runs `TextTemplateEngine.Process` when the
  text holds a `{`. The runtime pack already holds a shared string's key in `m_Key`
  (`wr-content`'s canon notes).
- **The engine** (`Kingmaker.TextTools/TextTemplateEngine.cs`): 59 registered tags, looked up
  lower-cased. `{tag}` and `{tag|p1|…}` call the template; an unknown tag (`{0}`, a
  `string.Format` slot) stays as written; an unterminated one is copied; `{}` and `{|…}`
  throw (`char.IsUpper` on an empty tag), which the game logs and rethrows to the caller. A
  tag is *capitalized* when its first character is upper case (`{Race}` keeps the race's
  case, `{race}` lower-cases it).
- **Templates dialogue uses**: `mf` (the gender of `GameLogContext.SourceUnit` in a log scope,
  else the dialogue's acting unit, else the main character; `PlayerIsKing` adds 2 with more
  than two parameters, and `BlueprintRoot.KingFlag` is null here), `name` (the acting unit's
  or the main character's `CharacterName`, itself a localized string), `race`, `n`/`/n` (the
  narrator colour), `g`/`d`/`m` and their ends (glossary links, coloured from
  `UIRoot.DialogColors`, or `BookEventColors` in a book event), `br`, `empty`, `pc_console`.
  Counted in the English pack [M]: `g` 41,307, `n` 18,522, `mf` 3,333, `name` 783, `d` 419,
  then the combat log's (`count`, `source`, `text`, `target`, `d20`…), `t`, `br`, `flag`,
  `bind`/`console_bind`, a few kingdom and UI ones.
- **Unit names** (`UnitDescriptor.CharacterName`): `ReplaceBlueprintForInspection`'s name, else
  the saved `CustomName`, else the blueprint's (`BlueprintUnit.LocalizedName`, a
  `SharedStringAsset`; `-unit name not set-` without one).
- **The dialogue's history** (`DialogController.SelectAnswer`): before anything else it
  raises a line for the current cue (`AddHistoryEntry(CurrentCue, CurrentSpeakerName,
  CurrentSpeakerColor)`) and one for the answer when `AddToHistory` (the main character's
  name and colour in a common dialogue), through `IDialogHandler.HandleOnDialogHistory`, in
  `DialogFormats`' shapes. `CurrentSpeakerName` is the cue's speaker portrait's name, else the
  current speaker's, else the start's custom speaker name; `CurrentSpeakerColor` is that
  blueprint's colour (`ColorUtility.ToHtmlStringRGB`), else the controller's
  `m_CustomSpeakerColor`. That field outlives dialogues and loads: `Clear` keeps it, a start
  sets it only when the dialogue starts with a speaker.

## What was built

- **`crates/wr-text`**: `pack` (the JSON read once, our format written to
  `cache/text/<locale>.wrtext`: keys sorted by their bytes, so lookups are the dictionary's
  exact ones; memory-mapped, every entry's bounds and the order checked at open, UTF-8 at
  lookup; rebuilt when the JSON's length or time changes; replaced by a rename, never in
  place), `engine` (the scan step for step, on bytes where the C# works on UTF-16 code units:
  every delimiter is ASCII, so they agree; the templates; an `Env` trait for the state
  templates read, each query unsupported by default, so text that needs unmodelled state fails
  closed), `display` (the `LocalizedString` conversion). `wr-text build|get|bench|tags|scan-generic`.
- **The shared strings' keys**: the runtime pack stores a `SharedStringAsset` field as its
  index into `BlueprintReferencedAssets` (`{"asset_index": i}`), and the zip's copy of the key
  can be stale (POC-2 measured 379 `LocalizedString` keys that differ). `tools/wr-rules-data`
  now also writes `cache/sim/shared-strings.json`: for each entry that is a
  `SharedStringAsset` (by its script, or by its shape when the script is in another file: a
  `MonoBehaviour` whose only own field is the `LocalizedString` `String`), the key its string
  resolves to. 4,457 of the 10,279 entries [M]; 2,220 entries point into other files and
  aren't resolved (an index the table lacks is reported).
- **The sim** (`crates/wr-sim/src/text.rs`): the template environment over the world
  (`mf`, `name`, `race`, the colours from `BlueprintRoot.UIRoot`), unit and blueprint names
  and colours, `DialogFormats`. The dialogue controller (`dialog/mod.rs`) keeps
  `m_CustomSpeakerName` and `m_CustomSpeakerColor` (the colour unknown until a start or the
  recording says), and, when the recording declares `dialog_text`, writes `dlg_history`
  (each line `SelectAnswer` raises, before its effects) and `dlg_text` (after each
  transition: the cue's and the shown answers' display text, the speaker's name and colour).
  Text the sim can't make is written as null and reported. `StartDialog` without a target
  makes its `SpeakerName` string as the start data is built: a template the game throws on
  stops the start (as the exception does), one the sim can't make is reported. Units keep their
  saved `CustomName` and `ReplaceBlueprintForInspection`.
- **WrathOracle** (`TextObs.cs`, declared `dialog_text`): `dlg_history` from the first of the
  line's subscribers (the game log's handler, the dialogue view model, the old dialogue UI)
  that receives it, between `AddHistoryEntry`'s prefix and postfix (null when none did);
  `dlg_text` right after `dlg_transition`, reading `DisplayText`, `CurrentSpeakerName` and
  `CurrentSpeakerColor` as the UI does when the cue shows; the begin record's
  `dialog_speaker` (the controller's two custom speaker fields when the window opens).
  `DisplayText` sets the string's `ShouldProcess` cache flag, as the UI's own read does.

## Results

- **Captures** `poc4-seelah-7`, `poc4-horgus-11`, `cert-24` (Manual_13, Manual_19, Manual_13),
  recorded with `dialog_text`: each run's stream has 22 to 30 more records than before (the
  history lines and text records), and every one equals the sim's [M]. The glossary links,
  narrator markup, speaker colours (`BlueprintUnit.Color` through Unity's rounding), the
  main character's name and colour on answers, and the speakers' names (shared strings
  resolved through the extracted table) all match. Every window opened with both custom
  speaker fields null.
- **The first captures** (`poc4-seelah-6`, `poc4-horgus-10`, `cert-23`, superseded) had no
  window-start speaker state: the sim reported the colour of the one cue without a speaker
  (unknown from before the window) where it was read, and the speakers' names were
  `-unit name not set-` until the shared strings were resolved from `blueprint.assets` (the
  runtime pack holds asset indices).
- **Fail closed** (`tests/poc4.rs`, `damaged_text_records_are_caught`): a changed history line,
  answer line, cue text, answer text, speaker name or colour, a dropped record, the declaration
  dropped while the records stay, or the speaker state missing or malformed: every one makes
  the run unequal or reported; the undamaged copy stays valid.
- **Casing**: `ToLowerInvariant` is Mono's table (`crates/wr-text/src/casing.rs`, generated
  from the game's `mscorlib`: 973 code units change; U+0130 and the Kelvin sign are kept, which
  CoreCLR's casing doesn't do, so the first harness, on CoreCLR, couldn't show it: review round
  2 did). No non-ASCII code unit lower-cases to ASCII, so only ASCII tags match.
- Reviews: gpt-6.1-sol (high). Round 1: a shared temporary path for concurrent pack writers,
  a malformed speaker state read as null, pack keys' UTF-8 unchecked. Round 2: `StartDialog`'s
  speaker name errors ignored (a game exception now stops the start, an unsupported template
  is reported), Mono's casing. Round 3: the text loaded only with a recording, the request
  record missing when the name throws (the game's hook records it first), the shared strings'
  output directory not created.

## Not done

- The templates of the kingdom, the UI and tutorials (`kingdomname`, `bind`, `t`, `ui`…), `flag`
  and `date`/`time` (no game clock text yet): the `Env` reports them. The combat log's (`count`,
  `source`, `d20`…) are ported: `docs/poc5/combat-log.md`. Voice-over (`Sound.json`'s events) is read but not played.
- Locales other than English: the oracle build runs in English; the packs load the same way.
- The shared strings in other files of the bundle (2,220 referenced entries).
- Not exercised by a capture yet (round 4's residual gaps): a non-null custom speaker state
  carried across dialogues, a cue's speaker portrait overriding the speaker, non-English text.
