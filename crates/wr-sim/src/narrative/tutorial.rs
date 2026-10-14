//! `ShowNewTutorial` → `TutorialSystem` (decompiled, not copied:
//! `Kingmaker.Tutorial.Actions/ShowNewTutorial.cs`, `Kingmaker.Tutorial/TutorialSystem.cs`,
//! `Tutorial.cs`).
//!
//! Ported: `Ensure(blueprint)` (creating the `Tutorial` fact, which takes a unique id from
//! the same counter as every other fact, so it shifts later ids) and the tag-ban check
//! against the save's `settings.game.tutorial.*`. **Stubbed (presentation):** `Trigger`
//! and the later `Tick`/`Show` (the candidate, its countdown and cooldown run on real time
//! and the UI; `ShowedTimes`/`LastShowIndex` counters), and `Tutorial.IsEnabled`, which
//! depends on `EtudeBracketEnableTutorials` retains the sim doesn't track. The new fact is
//! attached without activation. Our UI is new (Decision 6), so the tutorial pop-up itself
//! is out of T0; the stub is recorded in the coverage report.

use serde_json::json;
use wr_content::Guid;

use crate::facts::lifecycle as fl;
use crate::world::{FactExt, FactIdx, FactKind, World};

fn find(w: &World, g: Guid) -> Option<FactIdx> {
    let sys = w.player.tutorial?;
    w.entity(sys).facts.list.iter().copied().find(|f| w.fact(*f).blueprint == Some(g))
}

/// `BlueprintTutorial.GetTutorial()`: a tutorial may redirect to another
/// (`m_OverrideTutorial`?). Not used by the meetings' tutorials: returns the blueprint.
fn resolve(g: Guid) -> Guid {
    g
}

/// `TutorialSystem.Ensure(blueprint)`: the tutorial's fact, created if missing.
fn ensure(w: &mut World, g: Guid) -> Option<FactIdx> {
    let Some(sys) = w.player.tutorial else {
        w.coverage.unsupported("TutorialSystem.Ensure: no TutorialSystem");
        return None;
    };
    if let Some(f) = find(w, g) {
        return Some(f);
    }
    let Some(b) = w.content.get(g) else {
        w.coverage.unsupported("TutorialSystem.Ensure: tutorial blueprint missing");
        return None;
    };
    let f = fl::new_fact(w, FactKind::Other, "Tutorial", b);
    w.fact_mut(f).ext = FactExt::Tutorial {
        triggered_times: 0,
        showed_times: 0,
        last_show_index: 0,
        trigger_logic_counter: 0,
        banned: false,
    };
    // `Tutorial.OnAttach` → `UpdateIsEnabled` → `UpdateIsActive`: activation is left out
    // (stub, see the module note).
    fl::manager_add_attach(w, sys, f, true);
    w.coverage.stub("Tutorial.IsEnabled (EnabledByEtude) on attach");
    Some(f)
}

/// `ShowNewTutorial.RunAction()`.
pub fn show_new_tutorial(w: &mut World, tutorial: Guid) {
    let g = resolve(tutorial);
    let Some(f) = ensure(w, g) else { return };
    let tag_banned = is_tag_banned(w, f);
    if !tag_banned {
        w.coverage.stub("TutorialSystem.Trigger/Show (UI)");
        w.events.push("stub", json!({ "what": "tutorial_trigger", "tutorial": g.to_string() }));
    }
}

/// `Ensure(tutorial.GetTutorial()).EnableByEtude()` / `DisableByEtude()`
/// (`EtudeBracketEnableTutorials`): the fact is ensured and its `m_EnabledByEtude` counter
/// kept; `UpdateIsEnabled`'s activation of the tutorial (its trigger components, UI) is a
/// stub.
pub fn enable_by_etude(w: &mut World, tutorial: Guid, on: bool) {
    let g = resolve(tutorial);
    if ensure(w, g).is_none() {
        return;
    }
    let key = format!("tutorial:{g}.EnabledByEtude");
    if on {
        crate::components::etudes::retain(w, &key);
    } else {
        crate::components::etudes::release(w, &key);
    }
    w.coverage.stub("Tutorial.UpdateIsEnabled (tutorial triggers and UI)");
}

/// `Tutorial.IsBanned(fromTrigger: false)` → `Owner.IsTagBanned(Blueprint.Tag)`: the tag's
/// `settings.game.tutorial.<tag>` is off.
fn is_tag_banned(w: &mut World, f: FactIdx) -> bool {
    let tag = w.fact(f).bp.as_ref().map(|b| b.view().int("Tag")).unwrap_or(0);
    let key = match tag {
        0 => "basis",
        1 => "controls-basis",
        2 => "controls-advanced",
        3 => "gameplay-basic",
        4 => "gameplay-advanced",
        5 => "pathfinder-rules",
        6 => "crusade",
        7 => "armies",
        // `NoTag`: `GetSettingByTag` is null and `ShouldShowTag` returns true.
        8 => return false,
        _ => {
            w.coverage.unsupported("TutorialSystem.IsTagBanned: unknown tag");
            return false;
        }
    };
    match w.player.tutorial_settings.get(key) {
        Some(on) => !on,
        None => {
            w.coverage.unsupported(format!("TutorialSystem.IsTagBanned: setting {key} missing"));
            false
        }
    }
}
