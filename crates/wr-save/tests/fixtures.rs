//! Real-save tests (ignored by default; they read Ember's fixture saves, read-only):
//! `cargo nextest run -p wr-save --run-ignored only`. Each skips cleanly when its save
//! is missing.

use std::path::PathBuf;

use wr_save::{canon, format, json, report, zks};

fn fixture(prefix: &str) -> Option<PathBuf> {
    let dir = wr_paths::fixture_saves();
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix)))
        .collect();
    v.sort();
    v.into_iter().next()
}

/// Decodes a fixture: every document parses and decodes with nothing opaque, no unknown
/// property and no unresolved reference; then our format round-trips it exactly and the
/// canonical text says the same as the original.
fn check(prefix: &str) {
    let Some(path) = fixture(prefix) else {
        eprintln!("skipping: no {prefix} fixture");
        return;
    };
    let entries = zks::read(&path).expect("zks reads");
    zks::check_versions(&entries).expect("loads without JSON upgraders");
    let decoded = zks::decode_all(&entries);
    let stats = report::merged(&decoded);
    assert!(decoded.iter().all(|d| d.error.is_none()), "parse errors");
    assert_eq!(stats.values_untyped, 0, "opaque: {:?}", stats.opaque_at);
    assert!(stats.unknown_props.is_empty(), "{:?}", stats.unknown_props);
    assert!(stats.refs.keys().all(|k| *k == "resolved"), "{:?}", stats.refs);
    if let Some(bp) = report::resolve_blueprints(&stats.bp_uses) {
        assert_eq!((bp.dangling, bp.wrong_type), (0, 0), "{:?}", bp.dangling_examples);
    }
    for (d, e) in decoded.iter().zip(&entries) {
        let zks::Body::Doc(doc) = &d.body else { continue };
        let a = json::parse(&e.bytes).unwrap();
        assert_eq!(a.bom, doc.bom);
        canon::check_source(&a.root, &doc.root, &doc.tags)
            .unwrap_or_else(|err| panic!("{}: {err}", d.name));
        let st = d.stats.as_ref().unwrap();
        assert_eq!(st.values_typed + st.values_untyped, a.root.count(), "{}", d.name);
    }
    let items: Vec<format::Item> = decoded
        .iter()
        .map(|d| format::Item { name: d.name.clone(), body: d.body.clone() })
        .collect();
    let bytes = format::encode(&items).unwrap();
    let back = format::decode(&bytes).unwrap();
    assert_eq!(back.len(), items.len());
    for (x, y) in items.iter().zip(&back) {
        assert_eq!(x.name, y.name);
        match (&x.body, &y.body) {
            (zks::Body::Doc(a), zks::Body::Doc(b)) => {
                assert!(report::same(a, b), "{}", x.name);
                assert_eq!(canon::doc_text(a), canon::doc_text(b), "{}", x.name);
            }
            (zks::Body::Bytes(a), zks::Body::Bytes(b)) => assert_eq!(a, b, "{}", x.name),
            _ => panic!("{}: kind changed", x.name),
        }
    }
}

#[test]
#[ignore = "reads Ember's Manual_13 fixture"]
fn manual_13_decodes_and_round_trips() {
    check("Manual_13_");
}

#[test]
#[ignore = "reads Ember's Manual_10 fixture"]
fn manual_10_decodes_and_round_trips() {
    check("Manual_10_");
}
