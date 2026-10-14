//! The required-versus-implemented manifest (Decision 7): every blueprint component on the
//! hydrated units' facts (and their items' facts), with what the C# type needs at run time
//! (lifecycle overrides, event handlers, a runtime at all) and whether it's ported.

use std::collections::BTreeMap;

use serde::Serialize;
use wr_content::schema::{self, fact_hooks};

use crate::world::{EntityIdx, World};

#[derive(Debug, Default, Clone, Serialize)]
pub struct Row {
    #[serde(rename = "type")]
    pub ty: String,
    /// Components of this type on active facts / on all facts.
    pub on_active_facts: u64,
    pub on_all_facts: u64,
    /// The type has a per-fact runtime (`IRuntimeEntityFactComponentProvider`).
    pub runtime: bool,
    /// Lifecycle hooks the C# type overrides.
    pub hooks: Vec<&'static str>,
    /// Event-handler interfaces it declares.
    pub handlers: Vec<&'static str>,
    /// "implemented", "required" (has hooks or handlers, not ported), "inert" (no hooks,
    /// no handlers: data others read), or "known-inert: <why>".
    pub status: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Manifest {
    pub units: Vec<String>,
    pub facts: u64,
    pub active_facts: u64,
    pub components: u64,
    pub components_on_active_facts: u64,
    pub types: usize,
    pub implemented: usize,
    /// Ported, but some hooks or handlers the C# type has aren't.
    pub partial: usize,
    pub required_missing: usize,
    /// Data-only and known-inert types.
    pub inert: usize,
    /// Required-and-missing components on active facts (the count that matters).
    pub missing_on_active_facts: u64,
    pub rows: Vec<Row>,
}

/// Subscriber marker interfaces (no callbacks of their own).
fn is_marker(h: &str) -> bool {
    matches!(h, "ISubscriber" | "IGlobalSubscriber" | "IUnitSubscriber")
        || h.ends_with("RulebookSubscriber")
        || h.starts_with("IRulebookHandler<")
}

fn hook_names(bits: u32) -> Vec<&'static str> {
    fact_hooks::NAMES.iter().filter(|(b, _)| bits & b != 0).map(|(_, n)| *n).collect()
}

/// Builds the manifest over the units' facts and their items' facts.
#[must_use]
pub fn build(w: &World, units: &[EntityIdx]) -> Manifest {
    let mut rows: BTreeMap<&'static str, Row> = BTreeMap::new();
    let mut m = Manifest {
        units: units.iter().map(|u| w.entity(*u).unique_id.clone()).collect(),
        ..Manifest::default()
    };
    let mut owners: Vec<EntityIdx> = units.to_vec();
    for &u in units {
        if let Some(unit) = w.unit(u) {
            for it in &unit.inventory {
                if !owners.contains(it) {
                    owners.push(*it);
                }
            }
            for s in &unit.body.slots {
                if let Some(it) = s.item
                    && !owners.contains(&it)
                {
                    owners.push(it);
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for e in owners {
        for &f in &w.entity(e).facts.list {
            if !seen.insert(f) {
                continue;
            }
            let fact = w.fact(f);
            m.facts += 1;
            m.active_facts += u64::from(fact.is_active);
            let Some(bp) = &fact.bp else { continue };
            for (b, i) in crate::content::collect_components(&w.content, bp) {
                let obj = b.components_array()[i];
                let t = schema::ty(obj.ty);
                let row = rows.entry(t.short).or_insert_with(|| row_for(w, t));
                row.on_all_facts += 1;
                m.components += 1;
                if fact.is_active {
                    row.on_active_facts += 1;
                    m.components_on_active_facts += 1;
                }
            }
        }
    }
    finish(m, rows)
}

fn row_for(w: &World, t: &'static schema::TypeDef) -> Row {
    let runtime = t.has(schema::type_flags::RUNTIME_PROVIDER);
    let status = if let Some(l) = w.registry.component(t.short) {
        let custom = t.fact_hooks & fact_hooks::CUSTOM_RUNTIME != 0;
        let missing_hooks = if custom { 0 } else { t.fact_hooks & !l.implemented() };
        let missing_handlers: Vec<&str> = t
            .handlers
            .iter()
            .filter(|h| !is_marker(h))
            .filter(|h| !l.handlers().contains(h))
            .copied()
            .collect();
        if missing_hooks == 0 && missing_handlers.is_empty() {
            "implemented".to_owned()
        } else {
            let mut parts = hook_names(missing_hooks);
            parts.extend(missing_handlers);
            format!("partial: missing {}", parts.join(", "))
        }
    } else if let Some(why) = w.registry.inert_reason(t.short) {
        format!("known-inert: {why}")
    } else if t.fact_hooks != 0 || t.handlers.iter().any(|h| !is_marker(h)) {
        "required".to_owned()
    } else {
        "data-only".to_owned()
    };
    Row {
        ty: t.short.to_owned(),
        runtime,
        hooks: hook_names(t.fact_hooks),
        handlers: t.handlers.to_vec(),
        status,
        ..Row::default()
    }
}

fn finish(mut m: Manifest, rows: BTreeMap<&'static str, Row>) -> Manifest {
    let mut v: Vec<Row> = rows.into_values().collect();
    v.sort_by(|a, b| {
        b.on_active_facts
            .cmp(&a.on_active_facts)
            .then(b.on_all_facts.cmp(&a.on_all_facts))
            .then(a.ty.cmp(&b.ty))
    });
    m.types = v.len();
    m.implemented = v.iter().filter(|r| r.status == "implemented").count();
    m.partial = v.iter().filter(|r| r.status.starts_with("partial")).count();
    m.required_missing = v.iter().filter(|r| r.status == "required").count();
    m.inert =
        v.iter().filter(|r| r.status == "data-only" || r.status.starts_with("known-inert")).count();
    m.missing_on_active_facts =
        v.iter().filter(|r| r.status == "required").map(|r| r.on_active_facts).sum();
    m.rows = v;
    m
}

/// The same ranking over every fact in every document of a save (all areas, not only the
/// hydrated units): what Manual_13 as a whole would need.
#[must_use]
pub fn from_save(w: &World, save: &crate::hydrate::Save) -> Manifest {
    use wr_save::value::{Key, Value};
    let mut facts: Vec<(wr_content::Guid, bool)> = Vec::new();
    for e in &save.entries {
        let wr_save::zks::Body::Doc(doc) = &e.body else { continue };
        doc.root.walk(&mut |v| {
            let Value::Obj(o) = v else { return };
            let td = wr_save::schema::ty(o.ty);
            if !td.has(wr_save::schema::type_flags::FACT) {
                return;
            }
            let (mut g, mut active) = (None, false);
            for p in &o.props {
                let Key::Member(i) = p.key else { continue };
                match td.members.get(usize::from(i)).map(|m| m.json) {
                    Some("Blueprint") => g = crate::hydrate::graph::guid_of(&p.value),
                    Some("IsActive") => active = matches!(p.value, Value::Bool(true)),
                    _ => {}
                }
            }
            if let Some(g) = g {
                facts.push((g, active));
            }
        });
    }
    let mut rows: BTreeMap<&'static str, Row> = BTreeMap::new();
    let mut m = Manifest { units: vec!["<whole save>".into()], ..Manifest::default() };
    for (g, active) in facts {
        m.facts += 1;
        m.active_facts += u64::from(active);
        let Some(bp) = w.content.get(g) else { continue };
        for (b, i) in crate::content::collect_components(&w.content, &bp) {
            let t = schema::ty(b.components_array()[i].ty);
            let row = rows.entry(t.short).or_insert_with(|| row_for(w, t));
            row.on_all_facts += 1;
            m.components += 1;
            if active {
                row.on_active_facts += 1;
                m.components_on_active_facts += 1;
            }
        }
    }
    finish(m, rows)
}
