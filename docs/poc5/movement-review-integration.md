# Review of the driven replay's movement agent (gpt-6.1-sol, high, of commit 661b7ba)

The integration still accepts modeled outputs as inputs and leaves important state and ordering checks unenforced. All 15 tests in the existing prebuilt POC-5 suite passed, but its captures stop at poc5-tb-97, leaving this agent-enabled integration and its damaged-record cases uncovered.

Full review comments:

- [P1] Resynchronize at the real-time boundary, not at a later output — crates/wr-sim/src/combat/replay/agent.rs:311-319
  Every real-time entry marks every agent for unchecked adoption, and the marker persists until that agent's next state check, potentially hundreds of turn-based frames later. That check can be a `Stop` end state, so this adopts the output of turn-based code the sim just ran and suppresses any disagreement. Capture a synchronization snapshot at the real-time/turn-based boundary, or report missing state as unsupported; do not exempt an arbitrary subsequent output from validation.

- [P1] Invoke agent methods from the modeled command calls — crates/wr-sim/src/combat/replay/agent.rs:402-405
  The commands adapter still only validates `uc_call` and returns; its `MoveTo`, `Follow`, and `Stop` calls never invoke the agent. Instead, these independent entry records supply the agent's arguments, and `ag_call_entry` even counts `TickApproaching`'s follow as an outside input. Consequently, an omitted idle Stop span or agent arguments inconsistent with the validated command call need not be rejected. Have the command call run the agent immediately and pull its matching records; reserve independent entries for genuinely unmodeled callers.

- [P1] Answer downstream movement getters from the simulated agent — crates/wr-sim/src/combat/replay/agent.rs:422-424
  Even with `ag` enabled, the commands adapter's `Inputs::leaf` still returns recorded `moving`, `rmoving`, and `wants`, while the turn controller still takes `tt_ca.wants` and `nomore` from the recording. These are now outputs of the agent being modeled, so command starts, approach decisions, and turn endings remain driven by oracle movement state rather than the sim's. Compute these getters from the simulated agent, consuming necessary external `IsDone` reads in order, and compare their recorded answers.

- [P1] Require evidence for position changes across unticked frames — crates/wr-sim/src/combat/replay/agent.rs:560-569
  A gap in `ag_ctl` records does not establish that a unit was asleep or moved through roaming. This branch nevertheless accepts any recorded position after such a gap, including a missing controller execution or unexplained teleport, without reporting unsupported. Moreover, real-time controller entries never update `ag_last_ctl`. Apply only explicitly observed outside position writes; otherwise report the unexplained discrepancy instead of inferring its cause from elapsed frames.

- [P1] Limit FindPath adoption to the state that FindPath writes — crates/wr-sim/src/combat/replay/agent.rs:607-613
  For every turn-based `FindPath`, this replaces the entire simulated agent without checking either boundary state. The original method writes `m_RequestedFindPath`, not the agent's existing movement path, speed, destination, or velocity. An earlier simulation error in those fields can therefore disappear here without any unsupported report. Keep the request as a counted input if necessary, but update only its affected fields and validate the remaining state before and after the call.

- [P2] Bind request identities at path_start instead of the request end — crates/wr-sim/src/combat/replay/agent.rs:1253-1260
  `request_key` copies `ag_req_end.rq.path`, and `req_end` then compares the simulated request key against that same value, making the identity check tautological. The intervening `path_start` is processed without validating its key or endpoint, so contradictory issuance and end records are not rejected here. Consume and validate `path_start` at the request-creation point, bind its identity to the simulated request, and independently check the end record.

- [P2] Derive newly constructed ForcedPath errors independently — crates/wr-sim/src/combat/replay/agent.rs:705-710
  For `TickApproaching`, the modeled caller constructs a fresh `ForcedPath` from its points; that constructor sets `CompleteState` to Complete, so its error flag is known to be false. Looking ahead to the nested `ag_done.err` instead lets the recording choose the simulated completion branch rather than checking the constructed object's state. Preserve the known metadata for constructed paths and validate the nested completion's error/state; external path metadata should be established at the call boundary.

- [P2] Validate the controller's complete per-frame unit sequence — crates/wr-sim/src/combat/replay/agent.rs:556-558
  Placing each received entry in `Phase::Move` does not verify that the controller iterated the expected awake units, in order, or returned early for `WaitingForUI`. A resequenced trace can omit, duplicate, or reorder stationary units' entire controller spans without changing their state and escape detection. Track the expected batch and its gates, match each `ag_ctl` against it, and check for missing entries when the phase closes, as required by the driven-replay design.

- [P2] Check the agent state recorded after TickUnit — crates/wr-sim/src/combat/replay/agent.rs:582-587
  `ag_ctl_end` now contains the complete agent state, including `UpdateVelocity`'s result, but this code checks only position and orientation and never reads `e.s`. Changing just `ag_ctl_end.s.vel` is therefore invisible, and a real velocity error can remain unchecked until a later agent entry—or indefinitely on the final tick. Validate this snapshot after `movement::tick_unit` and refresh the last recorded state.

- [P2] Compare observation rows with computed movement positions — crates/wr-sim/src/combat/replay.rs:2830-2833
  With agent replay enabled, frame-prefix `obs u` records still fall through to `obs`, which merely copies their rows into `self.pos.last`. Their positions are never compared with the positions computed by the movement controller, despite those rows continuing to establish positions used by combat replay. A contradictory row for an uninvolved unit can pass unnoticed. Validate established turn-based observation positions against simulated units before accepting the rows, retaining explicit input handling only for unmodeled spans.

- [P2] Validate CanMoveInTurnBased's recorded return and delta — crates/wr-sim/src/combat/replay/agent.rs:235-239
  Every `canmovetb` postfix is discarded without checking its unit, frame, returned boolean, or resulting delta. Those outputs are not uniquely established by the eventual agent state: changing the postfix's `v` or `dt` alone leaves all existing comparisons unchanged. Expose the modeled method's return and delta through an end hook and consume exactly its corresponding postfix, validating both outputs.

- [P2] Read DontInterruptInTurnBasedMode from the modeled command — crates/wr-sim/src/combat/replay/agent.rs:920-924
  The comment says the commands model does not have this flag, but `commands::Cmd` already stores `dont_interrupt_tb` and uses it in `interrupt_all`. Returning `ag_tick.dontint` allows the agent to preserve or complete movement according to a flag inconsistent with that same modeled command. Resolve the unit's active move command, compute the nullable flag from it, and check the recorded value rather than counting it as an input.

- [P2] Reject SlowDown draws that escape an agent entry — crates/wr-sim/src/combat/replay/agent.rs:199-204
  The new site is recognized only inside agent pulls and skipped spans. An extra SlowDown draw reaching the main `record` dispatcher falls through silently: outside-rule draws are excluded from T0, and `SiteTape::finish` does not check leftovers. An appended extra draw can therefore leave the run valid. Add the same global unexpected-draw rejection used for approach and impatience draws, and enforce consumption for this site when agent replay is enabled.

- [P2] Validate AllAgents initialization and enable indices — crates/wr-sim/src/combat/replay/agent.rs:472-477
  `ag_init.i` is ignored while enabled agents are appended in record order, and `ag_enable.i` is likewise never checked against the resulting list. Duplicate initialization, reordered enabled entries with unchanged indices, or an impossible enable index can therefore be accepted even though `repath_all` relies on this membership. Validate initial uniqueness and contiguous enabled indices, disabled entries' sentinel indices, and each enable/disable operation's recorded resulting index.

- [P2] Distinguish legacy captures from missing agent initialization — crates/wr-sim/src/combat/replay.rs:1301-1304
  Removing all `ag_init` records from a modern capture and resequencing it disables agent replay entirely; the remaining agent records then fall through the generic observation handler without validation. Missing prerequisite observers can produce the same silent downgrade. Preserve legacy behavior through an explicit capability/version distinction, but require initialization and dependencies for captures advertising driven-agent support instead of deciding support from the presence of the records being checked.

- [P2] Check PreviousPosition before TickUnit overwrites it — crates/wr-sim/src/combat/replay/agent.rs:558-560
  `ag_ctl.prev` records the previous controller execution's `PreviousPosition`, but this method checks only `pos` before `movement::tick_unit` overwrites the simulated previous position. Consequently, the only recorded check of that computed field is ignored, and an incorrect `prev` record is accepted. Compare it with `unit.combat.prev_position` before running the tick, establishing the initial value separately where necessary.

- [P2] Check current-unit and acting observations separately — crates/wr-sim/src/combat/replay/agent.rs:1024-1030
  Comparing only `cur && acting` accepts contradictory observations whenever either conjunct is false. For example, while another unit owns an acting turn, changing the recorded `acting` to false still passes because `cur_acting` remains false. Both values are independently known from simulated turn state. Require typed booleans and compare `cur` and `acting` individually instead of validating only their conjunction.

- [P2] Reject missing or mistyped boolean leaf values — crates/wr-sim/src/combat/replay/agent.rs:1267-1271
  These adapters interpret every value other than JSON `true` as false, including missing fields, nulls, and strings. For a standing unit whose desired orientation already equals its orientation, removing `canrotate.v` can therefore be accepted without any subsequent state difference. The recording has not established false in that case. Parse required leaf booleans with `as_bool()` and return a desync on missing or invalid values; apply the same validation to the other boolean leaf adapters.