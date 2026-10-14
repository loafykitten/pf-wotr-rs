//! Fail-closed registries (Decision 7) and the coverage manifest.
//!
//! Every behaviour type the game data can name (blueprint components, actions, conditions,
//! evaluators) is looked up here by its content type. Executing a type with no
//! implementation never silently does nothing: it records an *unsupported execution*,
//! which makes the run invalid (and, later, blocks progression saves). Which lifecycle
//! hooks a component type actually overrides comes from the decompiled C# through
//! `wr-codegen` (`TypeDef::fact_hooks`), so a hook the original leaves empty is a correct
//! no-op, not a gap. The manifest also lists, for loaded facts, every component type that
//! is *required* (it has lifecycle overrides or event handlers) against what is
//! implemented, which catches gaps in code that never runs during a test.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use wr_content::schema::{self, TypeIdx, fact_hooks};

use crate::world::{CompRef, World};

/// A fact component's lifecycle hook (`EntityFactComponent.On*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Hook {
    Initialize,
    FactAttached,
    Activate,
    Deactivate,
    TurnOn,
    TurnOff,
    PostLoad,
    ApplyPostLoadFixes,
    Recalculate,
    Dispose,
    ViewDidAttach,
    ViewWillDetach,
    PreSave,
}

impl Hook {
    /// The `fact_hooks` bit of this hook.
    #[must_use]
    pub fn bit(self) -> u32 {
        match self {
            Self::Initialize => fact_hooks::INITIALIZE,
            Self::FactAttached => fact_hooks::FACT_ATTACHED,
            Self::Activate => fact_hooks::ACTIVATE,
            Self::Deactivate => fact_hooks::DEACTIVATE,
            Self::TurnOn => fact_hooks::TURN_ON,
            Self::TurnOff => fact_hooks::TURN_OFF,
            Self::PostLoad => fact_hooks::POST_LOAD,
            Self::ApplyPostLoadFixes => fact_hooks::APPLY_POST_LOAD_FIXES,
            Self::Recalculate => fact_hooks::RECALCULATE,
            Self::Dispose => fact_hooks::DISPOSE,
            Self::ViewDidAttach => fact_hooks::VIEW_DID_ATTACH,
            Self::ViewWillDetach => fact_hooks::VIEW_WILL_DETACH,
            Self::PreSave => fact_hooks::PRE_SAVE,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Initialize => "OnInitialize",
            Self::FactAttached => "OnFactAttached",
            Self::Activate => "OnActivate",
            Self::Deactivate => "OnDeactivate",
            Self::TurnOn => "OnTurnOn",
            Self::TurnOff => "OnTurnOff",
            Self::PostLoad => "OnPostLoad",
            Self::ApplyPostLoadFixes => "OnApplyPostLoadFixes",
            Self::Recalculate => "OnRecalculate",
            Self::Dispose => "OnDispose",
            Self::ViewDidAttach => "OnViewDidAttach",
            Self::ViewWillDetach => "OnViewWillDetach",
            Self::PreSave => "OnPreSave",
        }
    }
}

/// A blueprint component's runtime behaviour. Each method mirrors the delegate's
/// override of the same name; the defaults are only reached for hooks the C# type
/// overrides but the port hasn't (which the dispatcher reports), so an implementation
/// overrides exactly the hooks its C# type overrides.
pub trait ComponentLogic {
    /// C# type short name (for reports).
    fn type_name(&self) -> &'static str;
    /// Hooks this port implements (`Hook::bit` set). Must cover the C# type's overrides.
    fn implemented(&self) -> u32;
    /// Event-handler interfaces this port implements (names as in `TypeDef::handlers`).
    fn handlers(&self) -> &'static [&'static str] {
        &[]
    }
    fn hook(&self, w: &mut World, c: CompRef, h: Hook);
    /// A global or unit event (`EventBus.RaiseEvent<iface>`).
    fn event(&self, w: &mut World, c: CompRef, iface: &str) {
        let _ = (c, iface);
        w.coverage.unsupported(format!("{}: event handler not ported", self.type_name()));
    }
    /// A rulebook event, before (`about == true`) or after the rule's own trigger.
    fn rule(
        &self,
        w: &mut World,
        c: CompRef,
        rule: &mut dyn crate::rulebook::RuleEvent,
        about: bool,
    ) {
        let _ = (c, rule, about);
        w.coverage.unsupported(format!("{}: rulebook handler not ported", self.type_name()));
    }
}

/// The component, action, condition and evaluator registries.
#[derive(Default)]
pub struct Registry {
    components: HashMap<&'static str, Box<dyn ComponentLogic>>,
    /// Component types known to be inert at run time for the reasons given (their data
    /// is read by others, e.g. prerequisites by level-up), so they're not "missing".
    known_inert: HashMap<&'static str, &'static str>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("components", &self.components.len()).finish()
    }
}

impl Registry {
    /// Panics if the type already has a port: a second one would silently replace it.
    pub fn register(&mut self, c: Box<dyn ComponentLogic>) {
        let name = c.type_name();
        assert!(self.components.insert(name, c).is_none(), "component {name} registered twice");
    }

    pub fn note_inert(&mut self, ty: &'static str, why: &'static str) {
        self.known_inert.insert(ty, why);
    }

    #[must_use]
    pub fn component(&self, ty: &str) -> Option<&dyn ComponentLogic> {
        self.components.get(ty).map(AsRef::as_ref)
    }

    #[must_use]
    pub fn implemented_types(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.components.keys().copied().collect();
        v.sort_unstable();
        v
    }

    #[must_use]
    pub fn inert_reason(&self, ty: &str) -> Option<&'static str> {
        self.known_inert.get(ty).copied()
    }
}

/// What a component type needs at run time, from the decompiled C#.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    pub hooks: u32,
    pub handlers: usize,
    pub runtime: bool,
}

/// Requirements of a content type.
#[must_use]
pub fn needs(t: TypeIdx) -> Needs {
    let d = schema::ty(t);
    Needs {
        hooks: d.fact_hooks,
        handlers: d.handlers.len(),
        runtime: d.has(schema::type_flags::RUNTIME_PROVIDER),
    }
}

/// Executed and missing behaviour, for the run report.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Coverage {
    /// Hook executions by (component type, hook) that ran ported code.
    pub executed: BTreeMap<String, u64>,
    /// Unsupported executions by description (component type and hook, or the
    /// subsystem). Any entry makes the run invalid.
    pub unsupported: BTreeMap<String, u64>,
    /// The same, per unit being loaded (`context`).
    pub unsupported_by_unit: BTreeMap<String, BTreeMap<String, u64>>,
    /// The unit whose lifecycle is running (set by the loader).
    #[serde(skip)]
    pub context: Option<String>,
    /// Behaviour knowingly simplified because it is presentation or outside T0 (UI
    /// pop-ups, animation, camera), by description. Listed in reports; doesn't
    /// invalidate the run.
    pub stubbed: BTreeMap<String, u64>,
}

impl Coverage {
    pub fn executed(&mut self, what: impl Into<String>) {
        *self.executed.entry(what.into()).or_default() += 1;
    }

    pub fn unsupported(&mut self, what: impl Into<String>) {
        let what = what.into();
        // A debugging aid: with `WR_DEBUG_UNSUPPORTED` set, each report is printed as it happens
        // (a driven replay prints each frame's tick too).
        if std::env::var_os("WR_DEBUG_UNSUPPORTED").is_some() {
            eprintln!("DEBUG unsupported: {what}");
        }
        if let Some(c) = &self.context {
            *self
                .unsupported_by_unit
                .entry(c.clone())
                .or_default()
                .entry(what.clone())
                .or_default() += 1;
        }
        *self.unsupported.entry(what).or_default() += 1;
    }

    pub fn stub(&mut self, what: impl Into<String>) {
        *self.stubbed.entry(what.into()).or_default() += 1;
    }

    /// Whether nothing unsupported ran while loading this unit.
    #[must_use]
    pub fn unit_valid(&self, id: &str) -> bool {
        self.unsupported_by_unit.get(id).is_none_or(BTreeMap::is_empty)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.unsupported.is_empty()
    }
}

/// Runs hook `h` of component `c`: no-op if the C# type doesn't override it (the delegate
/// base's empty virtual), the port if there is one, otherwise an unsupported execution.
pub fn dispatch_hook(w: &mut World, c: CompRef, h: Hook) {
    let Some(ty) = w.component(c).ty() else {
        // No source blueprint component (not set up, or disposed): the delegate is null.
        return;
    };
    if crate::components::etudes::is_bracket(ty) {
        crate::components::etudes::runtime_hook(w, c, h);
        return;
    }
    let d = schema::ty(ty);
    let overridden = d.fact_hooks & h.bit() != 0;
    let custom = d.fact_hooks & fact_hooks::CUSTOM_RUNTIME != 0;
    let reg = w.registry.clone();
    let port = reg.component(d.short);
    // `OnRecalculate`'s default (`EntityFactComponent.OnRecalculate`): `OnTurnOff();
    // OnTurnOn();` — for delegates that don't override it, and for custom runtimes whose
    // port doesn't (their runtime classes inherit the base method).
    let default_recalc =
        if custom { port.is_some_and(|l| l.implemented() & h.bit() == 0) } else { !overridden };
    if h == Hook::Recalculate && default_recalc {
        dispatch_hook(w, c, Hook::TurnOff);
        dispatch_hook(w, c, Hook::TurnOn);
        return;
    }
    if !overridden && !custom {
        return;
    }
    match port {
        Some(logic) if logic.implemented() & h.bit() != 0 => {
            w.coverage.executed(format!("{}.{}", d.short, h.name()));
            logic.hook(w, c, h);
        }
        _ => w.coverage.unsupported(format!("{}.{}", d.short, h.name())),
    }
}
