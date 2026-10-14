use super::*;
use crate::canon;
use crate::node::Reason;
use crate::schema::Codec;

fn decode_value(codec: Codec, json: &str) -> (Node, Tally, Vec<RefUse>) {
    let mut tally = Tally::default();
    let cx = Cx::new(&mut tally);
    let mut de = serde_json::Deserializer::from_str(json);
    let n = NodeSeed { cx: &cx, codec }.deserialize(&mut de).unwrap();
    let refs = cx.take_refs();
    drop(cx);
    (n, tally, refs)
}

fn ty(name: &str) -> TypeIdx {
    schema::find(name).unwrap_or_else(|| panic!("{name} in schema"))
}

const FEATURE: &str = r#"{
  "AssetId": "6dbf169364e94e51b869ac4ed95d9487",
  "Data": {
    "$type": "cb208b98ceacca84baee15dba53b1979, BlueprintFeature",
    "PrototypeLink": "",
    "m_Overrides": [],
    "Components": [
      {
        "$type": "d79ad4ed064aa4f43ace0e9c238fb9b9, AddFacts",
        "m_Flags": 0,
        "name": "$AddFacts$b25ff5aa",
        "PrototypeLink": {"guid": "", "name": ""},
        "m_Overrides": [],
        "m_Facts": ["!bp_df5f416373a54abc8543963c0aaab670", "!bp_CB957479424E4C4DA378013B40F140E8"],
        "Dummy": null,
        "CasterLevel": 3,
        "MinDifficulty": "Story"
      },
      {
        "$type": "00000000000000000000000000000001, NotARealType",
        "m_Flags": 0,
        "Whatever": [1, 2, 3]
      }
    ],
    "Comment": "",
    "m_DisplayName": {"m_Key": "", "m_OwnerString": "x", "Shared": {"assetguid": "a", "stringkey": "k1"}},
    "Ranks": 2,
    "Groups": ["Feat", "CombatFeat"]
  },
  "Meta": {"ShadowDeleted": false}
}"#;

#[test]
fn polymorphic_blueprint_with_components() {
    let mut tally = Tally::default();
    let d = decode_jbp(FEATURE.as_bytes(), &mut tally).unwrap();
    assert_eq!(d.guid, Guid::parse("6dbf169364e94e51b869ac4ed95d9487"));
    assert_eq!(d.data_type, Some(ty("Kingmaker.Blueprints.Classes.BlueprintFeature")));
    let data = data(&d.wrapper).unwrap();
    assert!(data.tagged);
    let Some(Node::Seq(comps)) = data.get("Components") else { panic!("components") };
    // AddFacts decoded through its schema; its editor-only `m_Overrides` kept as extra.
    let add = comps[0].as_object().unwrap();
    assert_eq!(schema::ty(add.ty).short, "AddFacts");
    assert_eq!(add.get("CasterLevel"), Some(&Node::Int(3)));
    assert_eq!(add.get("MinDifficulty"), Some(&Node::Enum(0)));
    assert!(add.extra.iter().any(|(k, _)| &**k == "m_Overrides"));
    // Fields not in the JSON are marked absent, not invented.
    assert_eq!(add.get("DoNotRestoreMissingFacts"), Some(&Node::Absent));
    // The component with an unknown TypeId is kept whole, and decoding goes on.
    let Node::Opaque(o) = &comps[1] else { panic!("opaque") };
    assert!(matches!(&o.reason, Reason::UnknownTypeId(t) if t.contains("NotARealType")));
    assert_eq!(o.value["Whatever"], serde_json::json!([1, 2, 3]));
    assert_eq!(d.opaque, 1);
    // `Groups` is an enum array given by names.
    let Some(Node::Seq(groups)) = data.get("Groups") else { panic!("groups") };
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(|g| matches!(g, Node::Enum(v) if *v > 0)));
    // Typed references, with the field's target type; GUIDs are case-insensitive.
    assert_eq!(d.refs.len(), 2);
    let fact = ty("Kingmaker.Blueprints.Facts.BlueprintUnitFact");
    assert!(d.refs.iter().all(|r| r.target == fact));
    assert_eq!(d.refs[1].guid.to_string(), "cb957479424e4c4da378013b40f140e8");
    // Wrapper-level `Meta` is unknown to the runtime wrapper type and kept.
    let w = d.wrapper.as_object().unwrap();
    assert_eq!(w.extra.len(), 1);
    assert_eq!(tally.unknown_tags.len(), 1);
}

#[test]
fn type_tag_rules() {
    let feature = ty("Kingmaker.Blueprints.Classes.BlueprintFeature");
    let component = ty("Kingmaker.Blueprints.BlueprintComponent");
    // A `$type` that isn't assignable to the declared type is rejected.
    let (n, ..) = decode_value(
        Codec::Object(component),
        r#"{"$type": "cb208b98ceacca84baee15dba53b1979, BlueprintFeature", "Ranks": 1}"#,
    );
    assert!(matches!(&n, Node::Opaque(o) if matches!(o.reason, Reason::NotAssignable { .. })));
    // `$type` is metadata only as the first property; later it's an ordinary unknown
    // property and the declared type is used.
    let (n, ..) = decode_value(
        Codec::Object(feature),
        r#"{"Ranks": 4, "$type": "d79ad4ed064aa4f43ace0e9c238fb9b9, AddFacts"}"#,
    );
    let o = n.as_object().unwrap();
    assert_eq!(o.ty, feature);
    assert!(!o.tagged);
    assert_eq!(o.get("Ranks"), Some(&Node::Int(4)));
    assert_eq!(&*o.extra[0].0, "$type");
    // No tag on an abstract declared type: Newtonsoft can't create it.
    let action = ty("Kingmaker.ElementsSystem.GameAction");
    let (n, ..) = decode_value(Codec::Object(action), r#"{"name": "x"}"#);
    assert!(matches!(&n, Node::Opaque(o) if matches!(o.reason, Reason::AbstractWithoutTag(_))));
    assert!(matches!(&n, Node::Opaque(o) if o.value["name"] == "x"));
}

#[test]
fn references() {
    let fact = ty("Kingmaker.Blueprints.Facts.BlueprintUnitFact");
    let class = ty("Kingmaker.Blueprints.BlueprintUnitFactReference");
    let c = Codec::BpRef { target: fact, class };
    let (n, _, refs) = decode_value(c, r#""!bp_df5f416373a54abc8543963c0aaab670""#);
    assert_eq!(n, Node::BpRef(Guid::parse("df5f416373a54abc8543963c0aaab670").unwrap()));
    assert_eq!(
        refs,
        vec![RefUse {
            guid: Guid::parse("df5f416373a54abc8543963c0aaab670").unwrap(),
            target: fact
        }]
    );
    assert_eq!(decode_value(c, "null").0, Node::BpRef(Guid::EMPTY));
    assert_eq!(decode_value(c, r#""""#).0, Node::BpRef(Guid::EMPTY));
    // Text without the prefix is ignored by the converter (noted).
    let (n, t, refs) = decode_value(c, r#""df5f416373a54abc8543963c0aaab670""#);
    // Kept as written next to what the game reads.
    assert!(
        matches!(&n, Node::Coerced(inner, raw) if **inner == Node::BpRef(Guid::EMPTY) && raw.as_str() == Some("df5f416373a54abc8543963c0aaab670"))
    );
    assert_eq!(canon::value(c, &n), "null");
    assert!(refs.is_empty());
    assert_eq!(t.notes.len(), 1);
    // A bad GUID after the prefix reads as empty (`BlueprintGuid.Parse` logs it).
    assert!(matches!(decode_value(c, r#""!bp_nonsense""#).0, Node::Coerced(..)));
    // Anything but a string or null doesn't fit.
    assert!(decode_value(c, "12").0.is_opaque());
}

#[test]
fn unity_value_types() {
    use crate::schema::UnityKind as U;
    let v3 = Codec::Unity(U::Vector3);
    assert_eq!(
        decode_value(v3, r#"{"x": 1.5, "y": -2, "z": 0.25}"#).0,
        Node::Floats(vec![1.5, -2.0, 0.25].into())
    );
    // The save-format shorthand: integers over 100.
    assert_eq!(decode_value(v3, r#""150|-200|25""#).0, Node::Floats(vec![1.5, -2.0, 0.25].into()));
    assert!(decode_value(v3, "null").0.is_opaque());
    let color = Codec::Unity(U::Color);
    assert_eq!(
        decode_value(color, r#"{"r": 1.0, "g": 0.5, "b": 0.0, "a": 1.0}"#).0,
        Node::Floats(vec![1.0, 0.5, 0.0, 1.0].into())
    );
    // Color32 is a packed int, r in the low byte.
    let c32 = Codec::Unity(U::Color32);
    let (n, ..) = decode_value(c32, "-16776961");
    assert_eq!(canon::value(c32, &n), "[255, 0, 0, 255]");
    // Bounds: centre and size, each a Vector3 or its shorthand.
    let b = Codec::Unity(U::Bounds);
    assert_eq!(
        decode_value(b, r#"{"center": "100|0|-50", "size": {"x": 2, "y": 2, "z": 2}}"#).0,
        Node::Floats(vec![1.0, 0.0, -0.5, 2.0, 2.0, 2.0].into())
    );
    // Curves: float fields may be the strings Convert.ToSingle accepts.
    let curve = Codec::Unity(U::AnimationCurve);
    let (n, ..) = decode_value(
        curve,
        r#"{"preWrapMode": 8, "postWrapMode": 8, "keys": [
            {"time": 1.0, "value": 2.0, "inTangent": "Infinity", "outTangent": 0, "inWeight": 0.3, "outWeight": 0.3, "weightedMode": 0},
            {"time": 0.0, "value": 1.0, "inTangent": 0, "outTangent": 0, "inWeight": 0.3, "outWeight": 0.3, "weightedMode": 0},
            {"time": 0.0, "value": 9.0, "inTangent": 0, "outTangent": 0, "inWeight": 0.3, "outWeight": 0.3, "weightedMode": 0}]}"#,
    );
    let Node::Curve(c) = &n else { panic!("curve") };
    assert_eq!(c.keys[0].in_tangent, f32::INFINITY);
    // Canonical: sorted by time, the second key at t=0 dropped, wrap modes not kept.
    assert_eq!(
        canon::value(curve, &n),
        r#"{"keys": [[0, 1, 0, 0, 0.3, 0.3, 0], [1, 2, "Infinity", 0, 0.3, 0.3, 0]]}"#
    );
    let g = Codec::Unity(U::Gradient);
    let (n, ..) = decode_value(
        g,
        r#"{"colors": [{"time": 0, "r": 1, "g": 0, "b": 0}], "alpha": [{"time": 0, "a": 1}]}"#,
    );
    assert!(matches!(n, Node::Gradient(_)));
    let v2i = Codec::Unity(U::Vector2Int);
    assert_eq!(decode_value(v2i, r#"{"x": 3, "y": -4}"#).0, Node::Ints(vec![3, -4].into()));
    // `GetVector2Int`: missing components give zero.
    // (kept as written next to the zero the game reads).
    let (n, ..) = decode_value(v2i, r#"{"x": 3}"#);
    assert!(
        matches!(&n, Node::Coerced(inner, raw) if **inner == Node::Ints(vec![0, 0].into()) && raw["x"] == 3)
    );
    // Color goes through the field contract: a missing component is 0.
    let color = Codec::Unity(U::Color);
    assert_eq!(decode_value(color, r#"{"r": 1}"#).0, Node::Floats(vec![1.0, 0.0, 0.0, 0.0].into()));
    // Converter objects with keys the converter would ignore are kept whole.
    assert!(decode_value(v3, r#"{"x": 1, "y": 2, "z": 3, "w": 4}"#).0.is_opaque());
    // `$id` before `$type` is still metadata; the tag applies and `$id` is kept.
    let feature = ty("Kingmaker.Blueprints.Classes.BlueprintFeature");
    let simple = ty("Kingmaker.Blueprints.SimpleBlueprint");
    let (n, ..) = decode_value(
        Codec::Object(simple),
        r#"{"$id": "1", "$type": "cb208b98ceacca84baee15dba53b1979, BlueprintFeature", "Ranks": 2}"#,
    );
    let o = n.as_object().unwrap();
    assert_eq!(o.ty, feature);
    assert_eq!(&*o.extra[0].0, "$id");
}

#[test]
fn scalars_and_fallback() {
    let i32c = Codec::Int { bits: 32, signed: true };
    assert_eq!(decode_value(i32c, "-7").0, Node::Int(-7));
    // `ReadAsInt32` rejects float tokens, even integral ones, and parses numeric strings.
    assert!(decode_value(i32c, "1.0").0.is_opaque());
    assert!(matches!(decode_value(i32c, r#""12""#).0, Node::Coerced(i, _) if *i == Node::Int(12)));
    assert_eq!(decode_value(Codec::F32, r#""1.25""#).0, Node::F32(1.25));
    // A float read into a string would keep its JSON text, which we don't have exactly.
    assert!(decode_value(Codec::Str, "1.5").0.is_opaque());
    // Out of range or wrong kind: kept as opaque with the reason.
    let (n, t, _) = decode_value(i32c, "4294967296");
    assert!(matches!(&n, Node::Opaque(o) if o.value == serde_json::json!(4_294_967_296_u64)));
    assert_eq!(t.opaque_nodes, 1);
    let (n, ..) = decode_value(i32c, r#""seven""#);
    assert!(
        matches!(&n, Node::Opaque(o) if matches!(o.reason, Reason::Mismatch { codec: "int", json: "string" }))
    );
    // Newtonsoft turns a number into text for a string field.
    assert_eq!(decode_value(Codec::Str, "0").0, Node::Str("0".into()));
    assert_eq!(decode_value(Codec::Str, "null").0, Node::Null);
    // Floats: rounded to f32 from the double; non-finite as strings.
    assert_eq!(decode_value(Codec::F32, "0.1").0, Node::F32(0.1));
    assert_eq!(decode_value(Codec::F32, r#""-Infinity""#).0, Node::F32(f32::NEG_INFINITY));
    // Unsupported codecs keep the value.
    assert!(matches!(
        decode_value(Codec::Unsupported("X"), "1").0,
        Node::Opaque(o) if o.reason == Reason::Unsupported("X")
    ));
}

#[test]
fn enums() {
    let e = schema::enums().iter().position(|e| e.flags && e.members.len() > 2).unwrap();
    let def = &schema::enums()[e];
    let c = Codec::Enum(u32::try_from(e).unwrap());
    let (a, va) = def.members[1];
    let (b, vb) = def.members[2];
    assert_eq!(decode_value(c, &format!("\"{a}\"")).0, Node::Enum(va));
    assert_eq!(decode_value(c, &format!("\"{a}, {b}\"")).0, Node::Enum(va | vb));
    assert_eq!(decode_value(c, "12345").0, Node::Enum(12345));
    // `Enum.ToObject` wraps to the underlying int; `Enum.Parse` overflows to the default.
    assert!(matches!(decode_value(c, "4294967296").0, Node::Coerced(i, _) if *i == Node::Enum(0)));
    assert!(
        matches!(decode_value(c, r#""4294967296""#).0, Node::Coerced(i, _) if *i == Node::Enum(0))
    );
    assert!(decode_value(c, "1.0").0.is_opaque());
    assert_eq!(decode_value(c, "null").0, Node::Enum(0));
    // An unknown name reads as 0, as the game's converter does, and is noted.
    let (n, t, _) = decode_value(c, r#""NoSuchMember""#);
    assert!(matches!(&n, Node::Coerced(inner, _) if **inner == Node::Enum(0)));
    assert_eq!(canon::value(c, &n), "0");
    assert_eq!(t.notes.values().sum::<u64>(), 1);
}

#[test]
fn canonical_form() {
    let mut tally = Tally::default();
    let d = decode_jbp(FEATURE.as_bytes(), &mut tally).unwrap();
    let data = data(&d.wrapper).unwrap();
    let text = canon::blueprint(d.guid.unwrap(), "Example", data);
    let v: serde_json::Value = serde_json::from_str(&text).expect("canonical text is JSON");
    assert_eq!(v["$type"], "cb208b98ceacca84baee15dba53b1979");
    // `Ranks` from the JSON; a missing field gets its initializer.
    assert_eq!(v["Ranks"], 2);
    assert_eq!(v["IsClassFeature"], false);
    // Editor-only and authoring-only fields aren't in the runtime object.
    assert!(v.get("m_OwnerString").is_none());
    assert!(v["Components"][0].get("PrototypeLink").is_none());
    // The shared key moves into `m_Key`; `Shared` is null at run time.
    assert_eq!(v["m_DisplayName"]["m_Key"], "k1");
    assert!(v["m_DisplayName"]["Shared"].is_null());
    // A null non-blueprint object becomes a default instance; null strings are "".
    assert_eq!(v["m_DescriptionShort"]["m_Key"], "");
    assert_eq!(v["Components"][0]["m_Facts"][1], "cb957479424e4c4da378013b40f140e8");
    assert!(v["Components"][0]["Dummy"].is_null());
    // The opaque component is flagged, not dropped.
    assert!(v["Components"][1]["$unknown"].as_str().unwrap().contains("UnknownTypeId"));
    // Field order is the pack's (base class first).
    let order: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("  \""))
        .map(|l| l.split('"').next().unwrap())
        .collect();
    assert_eq!(&order[..4], ["guid", "name", "$type", "m_PrototypeId"]);
}

#[test]
fn pack_drops_what_the_game_drops() {
    // A byte field and a List<float>: the runtime pack writes no data for them.
    let n = Node::Int(200);
    assert_eq!(canon::value(Codec::Int { bits: 8, signed: false }, &n), "0");
    let floats = Codec::Seq { list: true, elem: &Codec::F32 };
    let v = Node::Seq(vec![Node::F32(1.0), Node::F32(2.0)]);
    assert_eq!(canon::value(floats, &v), "[0, 0]");
    let arr = Codec::Seq { list: false, elem: &Codec::F32 };
    assert!(canon::value(arr, &v).contains('2'));
}

/// Decodes the whole of Ember's `blueprints.zip`. Run with
/// `cargo nextest run -p wr-content --run-ignored only`.
#[test]
#[ignore = "needs the game's blueprints.zip"]
fn full_zip_decodes_through_schemas() {
    let paths = wr_paths::GamePaths::from_env();
    let zip = crate::source::BlueprintZip::open(&paths.blueprints_zip()).unwrap();
    let run = crate::report::run(&zip);
    let r = run.readability(&zip);
    assert_eq!(r.entries, 236_661);
    assert_eq!(r.errors, 0);
    assert_eq!(r.well_formed, r.entries);
    let c = run.coverage();
    assert_eq!(c.blueprints_typed, c.blueprints, "opaque reasons: {:?}", c.opaque_reasons);
    let refs = run.references();
    assert!(refs.resolved * 100 > refs.total * 99);
}
