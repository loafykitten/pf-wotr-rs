//! Decoder tests on small synthetic documents (type and member names from the generated
//! schema; no game data).

use super::*;
use crate::canon;
use crate::json;

fn ty(name: &str) -> TypeIdx {
    schema::by_name(name).unwrap_or_else(|| panic!("{name} not in schema"))
}

fn dec(name: &str, text: &str) -> (Doc, Stats) {
    let d = json::parse(text.as_bytes()).expect("test JSON parses");
    decode(&d, Some(ty(name)))
}

/// Decodes, checks the decoded values say exactly what the input said (and that the
/// canonical text parses to the same shape), checks every JSON value was counted once,
/// and returns the doc.
fn round(name: &str, text: &str) -> (Doc, Stats) {
    let (d, s) = dec(name, text);
    let a = json::parse(text.as_bytes()).unwrap().root;
    canon::check_source(&a, &d.root, &d.tags).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let b = json::parse(canon::doc_text(&d).as_bytes()).unwrap().root;
    assert_eq!(a.count(), b.count());
    assert_eq!(s.values_typed + s.values_untyped, a.count(), "every value counted once");
    (d, s)
}

fn refs(d: &Doc) -> Vec<(String, RefState)> {
    let mut out = Vec::new();
    d.root.walk(&mut |v| {
        if let Value::Ref(r) = v {
            out.push((r.id.text().into_owned(), r.state));
        }
    });
    out
}

fn opaque_reasons(d: &Doc) -> Vec<String> {
    let mut out = Vec::new();
    d.root.walk(&mut |v| {
        if let Value::Opaque(o) = v {
            out.push(o.reason.key());
        }
    });
    out
}

const MV: &str = "Kingmaker.EntitySystem.Stats.ModifiableValue";
const SKILL: &str = "Kingmaker.EntitySystem.Stats.ModifiableValueSkill, Assembly-CSharp";

#[test]
fn backward_forward_and_dangling_refs() {
    // m_Dependents[0] refers back to the root (a cycle), [1] forward to [2], [3] to
    // nothing.
    let text = format!(
        r#"{{"$id":"1","Type":"Strength","m_BaseValue":18,"m_Dependents":[
            {{"$id":"2","$type":"{SKILL}","m_BaseAttribute":{{"$ref":"1"}},"Type":"SkillAthletics"}},
            {{"$ref":"3"}},
            {{"$id":"3","$type":"{SKILL}","m_BaseAttribute":{{"$ref":"2"}}}},
            {{"$ref":"9"}}]}}"#
    );
    let (d, s) = round(MV, &text);
    assert_eq!(
        refs(&d),
        [
            ("1".into(), RefState::Resolved),
            ("3".into(), RefState::Forward),
            ("2".into(), RefState::Resolved),
            ("9".into(), RefState::Dangling),
        ]
    );
    assert_eq!(s.ids, 3);
    assert_eq!(s.values_untyped, 0, "{:?}", s.opaque);
    let root = d.root.as_obj().unwrap();
    assert_eq!(root.id, Some(Id::Num(1)));
    let deps = &root.props[2].value;
    let Value::Seq(items) = deps else { panic!() };
    let first = items[0].as_obj().unwrap();
    assert_eq!(schema::ty(first.ty).short(), "ModifiableValueSkill");
    assert_eq!(d.tags.len(), 1, "one distinct $type string");
}

#[test]
fn creator_with_parameters_registers_its_id_late() {
    // EntityFactsManager is built through `[JsonConstructor] (EntityDataBase owner)`:
    // its `$id` exists only after its properties are read, so a reference to it from
    // inside is null in the game.
    let text = r#"{"$id":"1","m_Facts":[{"$id":"2","$type":"Kingmaker.UnitLogic.Feature, Assembly-CSharp","SourceFact":{"$ref":"1"},"UniqueId":"A"},{"$ref":"2"}]}"#;
    let (d, _) = round("Kingmaker.EntitySystem.EntityFactsManager", text);
    assert_eq!(refs(&d), [("1".into(), RefState::Forward), ("2".into(), RefState::Resolved)]);
}

#[test]
fn polymorphic_members_and_type_errors() {
    // A reference inside a polymorphic list element, an unknown type, a type that isn't
    // assignable, and an abstract declared type without `$type`.
    let text = r#"{"$id":"1","m_Facts":[
        {"$id":"2","$type":"Kingmaker.UnitLogic.Buffs.Buff, Assembly-CSharp","SourceFact":{"$id":"3","$type":"Kingmaker.UnitLogic.Feature, Assembly-CSharp"}},
        {"$id":"4","$type":"Kingmaker.UnitLogic.Feature, Assembly-CSharp","SourceFact":{"$ref":"3"}},
        {"$type":"Kingmaker.NoSuchFact, Assembly-CSharp","x":1},
        {"$type":"Kingmaker.EntitySystem.Stats.ModifiableValue, Assembly-CSharp"},
        {"$type":"Kingmaker.UnitLogic.Feature, Other-Assembly"}]}"#;
    let (d, s) = round("Kingmaker.EntitySystem.EntityFactsManager", text);
    assert_eq!(refs(&d), [("3".into(), RefState::Resolved)]);
    let reasons = opaque_reasons(&d);
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(reasons[0].starts_with("unknown $type Kingmaker.NoSuchFact"));
    assert!(reasons[1].starts_with("not assignable"));
    assert!(reasons[2].starts_with("wrong assembly"));
    assert!(s.values_untyped > 0);
    let (d2, _) = dec(MV, r#"{"m_Dependents":[{"Type":"Strength"}]}"#);
    assert!(opaque_reasons(&d2).is_empty(), "ModifiableValue is concrete");
    let (d3, _) =
        dec("Kingmaker.EntitySystem.EntityFactsManager", r#"{"m_Facts":[{"UniqueId":"A"}]}"#);
    assert!(opaque_reasons(&d3).is_empty(), "EntityFact itself is concrete");
}

#[test]
fn metadata_rules() {
    // `$ref` must be alone; only leading `$` properties are metadata; `$type` first is
    // kept in that order.
    let (d, _) = dec(MV, r#"{"m_Dependents":[{"$ref":"1","Type":"Strength"}]}"#);
    assert_eq!(opaque_reasons(&d), ["bad metadata: $ref with other properties"]);
    let text = format!(
        r#"{{"m_Dependents":[{{"$type":"{SKILL}","$id":"5","Type":"Strength","$id":"6"}}]}}"#
    );
    let (d, s) = round(MV, &text);
    let Value::Seq(items) = &d.root.as_obj().unwrap().props[0].value else { panic!() };
    let o = items[0].as_obj().unwrap();
    assert!(o.type_first);
    assert_eq!(o.id, Some(Id::Num(5)));
    assert!(matches!(&o.props[1].key, Key::Unknown(k) if &**k == "$id"), "late $id is a property");
    assert_eq!(s.unknown_props.len(), 1);
}

#[test]
fn unknown_members_are_kept_and_counted() {
    let (d, s) = round(MV, r#"{"Type":"Strength","m_NotAField":{"deep":[1,2]},"m_basevalue":3}"#);
    let o = d.root.as_obj().unwrap();
    assert!(matches!(&o.props[1].key, Key::Unknown(k) if &**k == "m_NotAField"));
    assert!(matches!(&o.props[2].key, Key::MemberCase(_, k) if &**k == "m_basevalue"));
    assert_eq!(s.values_untyped, 4);
    let st = &s.per_type[&ty(MV)];
    assert_eq!((st.objects, st.clean, st.props_unknown), (1, 0, 1));
}

#[test]
fn vectors_and_nullable_vector() {
    // Vector3Converter text in hundredths, a non-canonical spelling, and Nullable<Vector3>
    // written through Vector3's own contract with a `Nullable` `$type`.
    let text = r#"{"$id":"1","Blueprint":"3bbcb27f6a0e6484b984f562c10cea29",
        "Position":{"$id":"2","$type":"System.Nullable`1[[UnityEngine.Vector3, UnityEngine.CoreModule]], mscorlib","x":-60.3882751,"y":0.0,"z":-4.33909225,"normalized":"-99|0|-7","magnitude":60.5439644,"sqrMagnitude":3665.57153},
        "FinalPosition":"11682|3930|-14093","EulerAngels":" 1|02|3"}"#;
    let (d, s) = round("Kingmaker.RandomEncounters.CombatRandomEncounterData", text);
    assert!(opaque_reasons(&d).is_empty(), "{:?}", opaque_reasons(&d));
    let o = d.root.as_obj().unwrap();
    let pos = o.props[1].value.as_obj().unwrap();
    assert_eq!(schema::ty(pos.ty).clr, "UnityEngine.Vector3");
    assert!(matches!(pos.props[0].value, Value::F32(x) if x == -60.388_275_f32));
    assert!(matches!(&pos.props[3].value, Value::Cm(c) if **c == [-99, 0, -7]));
    assert!(matches!(&o.props[2].value, Value::Cm(c) if **c == [11682, 3930, -14093]));
    assert!(matches!(&o.props[3].value, Value::Coerced(_)));
    assert_eq!(
        s.coerced.get("QuaternionConverter").copied().unwrap_or(0)
            + s.coerced.get("Vector3Converter").copied().unwrap_or(0),
        1
    );
    // Skipped (read-only) members: normalized, magnitude, sqrMagnitude.
    assert_eq!(s.skipped_props, 3);
    let (d, _) = dec(
        "Kingmaker.RandomEncounters.CombatRandomEncounterData",
        r#"{"FinalPosition":{"x":1,"y":2.5,"z":-3}}"#,
    );
    assert!(
        matches!(&d.root.as_obj().unwrap().props[0].value, Value::Floats(f) if **f == [1.0, 2.5, -3.0])
    );
    let (d, _) =
        dec("Kingmaker.RandomEncounters.CombatRandomEncounterData", r#"{"FinalPosition":"1|2"}"#);
    assert_eq!(opaque_reasons(&d), ["converter: vector text"]);
}

#[test]
fn enums() {
    // Names, numbers, a flags list, an unknown name (read as 0 by StringEnumConverter).
    let (d, s) = round(MV, r#"{"Type":"Strength"}"#);
    assert!(
        matches!(d.root.as_obj().unwrap().props[0].value, Value::Enum { name, .. } if name != ENUM_NUMBER)
    );
    let (d, _) = round(MV, r#"{"Type":3}"#);
    assert!(matches!(
        d.root.as_obj().unwrap().props[0].value,
        Value::Enum { name: ENUM_NUMBER, value: 3, .. }
    ));
    let (d, s2) = round(MV, r#"{"Type":"NotAStat"}"#);
    assert!(
        matches!(&d.root.as_obj().unwrap().props[0].value, Value::Coerced(c) if matches!(c.0, Value::Enum { value: 0, .. }))
    );
    assert_eq!(s2.coerced.get("enum"), Some(&1));
    assert_eq!(s.values_untyped, 0);
    let (d, _) = dec(MV, r#"{"Type":1.5}"#);
    assert_eq!(opaque_reasons(&d), ["float for enum"]);
    // Flags enum written as a list (SpellDescriptor is [Flags]).
    let mc = "Kingmaker.UnitLogic.Mechanics.MechanicsContext";
    let sd = schema::ty(ty(mc)).members.iter().find(|m| m.json == "SpellDescriptor").unwrap();
    let Codec::Enum(e) = sd.codec else { panic!() };
    let def = schema::enum_def(e);
    let names: Vec<&str> = def.members.iter().filter(|m| m.1 > 0).take(2).map(|m| m.0).collect();
    let text = format!(r#"{{"SpellDescriptor":"{}, {}"}}"#, names[0], names[1]);
    let (d, _) = round(mc, &text);
    assert!(matches!(&d.root.as_obj().unwrap().props[0].value, Value::EnumText(_)));
}

#[test]
fn dictionaries() {
    // Array form `[{Key, Value}]` with blueprint keys, a malformed entry the converter
    // skips, and a repeated key.
    let text = r#"{"$id":"1","m_UnlockedFlags":[
        {"Key":"ddfedbdeab95ed941b6968b06162c921","Value":1000},
        {"Value":0,"Key":"d15742b0a6c73ec4eaf1bac00f939782"},
        {"Key":"ddfedbdeab95ed941b6968b06162c921","Value":2}]}"#;
    let (d, s) = round("Kingmaker.AreaLogic.QuestSystem.UnlockableFlagsManager", text);
    let Value::Dict(dict) = &d.root.as_obj().unwrap().props[0].value else { panic!() };
    assert_eq!(dict.form, DictForm::Array);
    assert!(matches!(dict.entries[0], Entry::Pair(Value::GuidN(_), Value::Int(1000))));
    // `item["Key"]` / `item["Value"]` are read by name: another order is still an entry.
    assert!(matches!(dict.entries[1], Entry::Laid(Value::GuidN(_), Value::Int(0), _)));
    assert_eq!((s.dict_dropped, s.dict_repeats), (0, 1));
    assert_eq!(s.bp_uses.len(), 3);
    assert!(s.bp_uses.iter().all(|u| u.converter && u.in_dict));
    // Object form keyed by enum names (MechanicsContext.PropertiesConverter).
    let mc = "Kingmaker.UnitLogic.Mechanics.MechanicsContext";
    let pc = schema::ty(ty(mc)).members.iter().find(|m| m.json == "PropertiesConverter").unwrap();
    let Codec::Dict { key: Codec::Enum(e), .. } = pc.codec else { panic!() };
    let k = schema::enum_def(*e).members[0].0;
    let (d, _) = round(mc, &format!(r#"{{"PropertiesConverter":{{"{k}":4}}}}"#));
    let Value::Dict(dict) = &d.root.as_obj().unwrap().props[0].value else { panic!() };
    assert_eq!(dict.form, DictForm::Object);
    let (d, _) = dec(mc, r#"{"PropertiesConverter":{"NoSuchName":4}}"#);
    assert_eq!(opaque_reasons(&d), ["converter: dictionary enum key"]);
    // Object form with string keys and polymorphic values: fact components.
    let text = r#"{"$id":"1","Blueprint":"4391e8b9afbb0cf43aeba700c089f56d","AttachTime":"1.02:03:04.5000000","Components":{"$AddFacts$2912a6bc(Clone)":{"$id":"2","$type":"Kingmaker.UnitLogic.FactLogic.AllowDyingCondition+Runtime, Assembly-CSharp"},"$Other$1":null}}"#;
    let (d, s) = round("Kingmaker.EntitySystem.EntityFact", text);
    assert!(opaque_reasons(&d).is_empty(), "{:?}", opaque_reasons(&d));
    let o = d.root.as_obj().unwrap();
    assert!(matches!(o.props[1].value, Value::TimeSpan(t) if t == 937_845_000_000));
    assert_eq!(s.ids, 2);
}

#[test]
fn blueprints_references_and_scalars() {
    let text = r#"{"$id":"1","m_DlcRewards":["0123456789abcdef0123456789abcdef",{"guid":"00000000000000000000000000000001"},null,""],"Name":"n","QuestWithSaveDescription":"null","GameId":"738691be0e8e43bbb308eebe1b61e62c","Campaign":"fd2e11eb-b8a1-4d65-9945-0fc27f03486a","Type":"Manual","PartyPortraits":[],"SystemSaveTime":"2026-10-10T03:01:39.447702-05:00","GameSaveTime":"12:03:41.3070000","GameTotalTime":"00:05:28.9660000","Versions":[10,20],"CompatibilityVersion":2,"EndlessDelveSeed":-646464952}"#;
    let (d, s) = round("Kingmaker.EntitySystem.Persistence.SaveInfo", text);
    assert!(opaque_reasons(&d).is_empty(), "{:?}", opaque_reasons(&d));
    let o = d.root.as_obj().unwrap();
    let Value::Seq(r) = &o.props[0].value else { panic!() };
    assert!(matches!(r[0], Value::GuidN(_)));
    assert!(matches!(&r[1], Value::Coerced(_)));
    assert!(matches!(r[3], Value::GuidEmpty));
    assert!(matches!(o.props[2].value, Value::NullText));
    assert!(matches!(o.props[4].value, Value::GuidD(_)));
    assert!(matches!(o.props[7].value, Value::Date(_)));
    assert_eq!(s.bp_uses.iter().filter(|u| !u.converter).count(), 2);
    // Int range and spelling.
    let (d, _) =
        dec("Kingmaker.EntitySystem.Persistence.SaveInfo", r#"{"QuickSaveNumber":2147483648}"#);
    assert_eq!(opaque_reasons(&d), ["converter: integer out of range"]);
    let (d, _) = dec("Kingmaker.EntitySystem.Persistence.SaveInfo", r#"{"QuickSaveNumber":1.0}"#);
    assert_eq!(opaque_reasons(&d), ["float for integer"]);
    let (d, _) = dec("Kingmaker.EntitySystem.Persistence.SaveInfo", r#"{"QuickSaveNumber":"1"}"#);
    assert_eq!(opaque_reasons(&d), ["string for integer"]);
}

#[test]
fn entity_refs_and_floats() {
    let mc = "Kingmaker.UnitLogic.Mechanics.MechanicsContext";
    let text = r#"{"AssociatedBlueprint":"4391e8b9afbb0cf43aeba700c089f56d","m_OwnerRef":"35fffa0a-06ba-4cba-be40-7914924be190","m_CasterRef":{"m_UniqueId":"x"},"m_MainTarget":{"$id":"13","UnitRef":"35fffa0a-06ba-4cba-be40-7914924be190","m_Point":"0|0|0","m_Orientation":null},"m_Params":null,"m_ShadowFactorPercents":0,"Direction":"0|0|100"}"#;
    let (d, s) = round(mc, text);
    assert!(opaque_reasons(&d).is_empty(), "{:?}", opaque_reasons(&d));
    assert_eq!(s.entity_refs.len(), 3);
    assert!(matches!(&d.root.as_obj().unwrap().props[2].value, Value::Coerced(_)));
    // Floats: lexemes read as double then rounded; NaN as a string.
    let v = "UnityEngine.Vector3";
    let (d, _) = round(v, r#"{"x":-161.616272,"y":"NaN","z":1E-05}"#);
    let o = d.root.as_obj().unwrap();
    assert!(
        matches!(o.props[0].value, Value::F32(x) if x == ("-161.616272".parse::<f64>().unwrap() as f32))
    );
    assert!(matches!(o.props[1].value, Value::F32(x) if x.is_nan()));
}

#[test]
fn skipped_members_hide_their_ids() {
    // `normalized` on Vector3's contract is read-only: the game skips it, so an `$id`
    // inside it would never be registered (constructed example).
    let text = r#"{"$id":"1","Position":{"$id":"2","$type":"System.Nullable`1[[UnityEngine.Vector3, UnityEngine.CoreModule]], mscorlib","x":1.0,"y":0.0,"z":0.0}}"#;
    let (d, _) = round("Kingmaker.RandomEncounters.CombatRandomEncounterData", text);
    assert!(opaque_reasons(&d).is_empty());
}

#[test]
fn duplicate_ids_fail_like_newtonsoft() {
    let text = format!(
        r#"{{"$id":"1","m_Dependents":[{{"$id":"2","$type":"{SKILL}"}},{{"$id":"2","$type":"{SKILL}"}},{{"$ref":"2"}}]}}"#
    );
    let (d, _) = round(MV, &text);
    assert_eq!(opaque_reasons(&d), ["bad metadata: duplicate $id"]);
}

#[test]
fn failed_dictionary_key_skips_the_value() {
    // `UnitBody.m_UnusedAdditionalLimbs`-style array dictionary: a key that fails to
    // convert makes the converter skip the entry, so ids in its value never register.
    let t = schema::types()
        .iter()
        .enumerate()
        .find_map(|(i, t)| {
            let m = t.members.iter().position(|m| {
                matches!(m.codec, Codec::Dict { key: Codec::Blueprint(_), value: Codec::Object(_) })
            })?;
            Some((i, m))
        })
        .expect("a dictionary with blueprint keys and object values");
    let (ti, mi) = t;
    let td = schema::ty(u32::try_from(ti).unwrap());
    let member = td.members[mi].json;
    let Codec::Dict { value: Codec::Object(vt), .. } = td.members[mi].codec else { panic!() };
    let vty = schema::ty(*vt);
    if vty.has(type_flags::ABSTRACT) {
        return; // the value needs a concrete type; this schema's first match has none
    }
    let text = format!(r#"{{"{member}":[{{"Key":{{}},"Value":{{"$id":"ghost"}}}}]}}"#);
    let (d, s) = round(td.clr, &text);
    let Value::Dict(dict) = &d.root.as_obj().unwrap().props[0].value else { panic!() };
    assert!(matches!(dict.entries[0], Entry::Laid(Value::Opaque(_), Value::Json(_), _)));
    assert_eq!(s.dict_dropped, 1);
    assert_eq!(s.ids, 0, "the value's $id is not registered");
}

#[test]
fn numeric_and_nullable_edge_cases() {
    // An octal float (Newtonsoft reads 010 as 8), an enum integer that wraps, an
    // impossible date, and an invalid name for a Nullable<enum> (read as null).
    let (d, _) = dec("UnityEngine.Vector3", r#"{"x":010,"y":0,"z":0}"#);
    assert_eq!(opaque_reasons(&d), ["converter: leading-zero (octal) number"]);
    let (d, s) = round(MV, r#"{"Type":4294967297}"#);
    assert!(
        matches!(&d.root.as_obj().unwrap().props[0].value, Value::Coerced(c) if matches!(c.0, Value::Enum { value: 1, .. }))
    );
    assert_eq!(s.coerced.get("enum"), Some(&1));
    let (d, _) = dec(
        "Kingmaker.EntitySystem.Persistence.SaveInfo",
        r#"{"SystemSaveTime":"2026-99-99T99:99:99"}"#,
    );
    assert_eq!(opaque_reasons(&d), ["string for DateTime"]);
    let (d, _) = dec(
        "Kingmaker.EntitySystem.Persistence.SaveInfo",
        r#"{"SystemSaveTime":"2024-02-29T23:59:59.1234567+05:30"}"#,
    );
    assert!(opaque_reasons(&d).is_empty());
    let nullable_enum = schema::types().iter().find_map(|t| {
        let m = t.members.iter().find(|m| matches!(m.codec, Codec::Nullable(Codec::Enum(_))))?;
        Some((t.clr, m.json))
    });
    if let Some((clr, member)) = nullable_enum
        && !schema::ty(ty(clr)).has(type_flags::ABSTRACT)
    {
        let (d, _) = round(clr, &format!(r#"{{"{member}":"NotAValue"}}"#));
        let v = &d.root.as_obj().unwrap().props[0].value;
        assert!(matches!(v, Value::Coerced(c) if matches!(c.0, Value::Null)), "{v:?}");
    }
}

#[test]
fn type_name_assemblies() {
    // A game type without an assembly can't be bound (Type.GetType only searches
    // Newtonsoft and mscorlib); a generic argument from the wrong assembly neither.
    let (d, _) = dec(
        MV,
        r#"{"m_Dependents":[{"$type":"Kingmaker.EntitySystem.Stats.ModifiableValueSkill"}]}"#,
    );
    assert!(opaque_reasons(&d)[0].starts_with("wrong assembly"));
    let text = r#"{"$id":"1","Position":{"$type":"System.Nullable`1[[UnityEngine.Vector3, MissingAssembly]], mscorlib","x":1.0,"y":0.0,"z":0.0}}"#;
    let (d, _) = dec("Kingmaker.RandomEncounters.CombatRandomEncounterData", text);
    assert!(opaque_reasons(&d)[0].starts_with("wrong assembly"), "{:?}", opaque_reasons(&d));
}

#[test]
fn constructor_parameters_are_read_without_setters() {
    // GlobalMapArmyState.ArmyType has no setter but feeds its [JsonConstructor]'s
    // `armyType` parameter; Vector3's `magnitude` has neither and is skipped.
    let t = ty("Kingmaker.Globalmap.State.GlobalMapArmyState");
    let m = schema::ty(t).members.iter().find(|m| m.json == "ArmyType").unwrap();
    assert!(m.flags & member_flags::CTOR_PARAM != 0);
    assert!(schema::ty(t).has(type_flags::CREATOR_PARAMS));
    let (_, s) = round("UnityEngine.Vector3", r#"{"x":1.0,"y":0.0,"z":0.0,"magnitude":1.0}"#);
    assert_eq!(s.skipped_props, 1);
}

#[test]
fn second_review_cases() {
    // Octal through the vector converter's object form.
    let enc = "Kingmaker.RandomEncounters.CombatRandomEncounterData";
    let (d, _) = dec(enc, r#"{"FinalPosition":{"x":010,"y":0,"z":0}}"#);
    assert_eq!(opaque_reasons(&d), ["converter: vector object shape"]);
    // A child taking the id of a constructor-with-parameters parent registered later.
    let text = r#"{"$id":"x","m_Facts":[{"$id":"x","$type":"Kingmaker.UnitLogic.Feature, Assembly-CSharp"}]}"#;
    let (d, _) = round("Kingmaker.EntitySystem.EntityFactsManager", text);
    assert_eq!(opaque_reasons(&d), ["bad metadata: duplicate $id"]);
    // Repeated properties deep inside a converter-loaded dictionary.
    let mc = "Kingmaker.UnitLogic.Mechanics.MechanicsContext";
    let pc = schema::ty(ty(mc)).members.iter().find(|m| m.json == "PropertiesConverter").unwrap();
    let Codec::Dict { key: Codec::Enum(e), .. } = pc.codec else { panic!() };
    let k = schema::enum_def(*e).members[0].0;
    let (d, _) = round(mc, &format!(r#"{{"PropertiesConverter":{{"{k}":[{{"a":1,"a":2}}]}}}}"#));
    assert_eq!(
        opaque_reasons(&d),
        ["converter: repeated property inside a converter-loaded value"]
    );
    // Object form: a failed value skips its entry before the key is parsed.
    let (d, s) = round(mc, r#"{"PropertiesConverter":{"NoSuchName":"x"}}"#);
    assert_eq!(opaque_reasons(&d), ["string for integer"]);
    assert_eq!(s.dict_dropped, 1);
    // Array form: a missing Value still converts the key; extra properties are counted.
    let flags = "Kingmaker.AreaLogic.QuestSystem.UnlockableFlagsManager";
    let text = r#"{"m_UnlockedFlags":[{"Key":"ddfedbdeab95ed941b6968b06162c921"},{"Key":"d15742b0a6c73ec4eaf1bac00f939782","Value":1,"Extra":{"x":[1,2]}}]}"#;
    let (d, s) = round(flags, text);
    let Value::Dict(dict) = &d.root.as_obj().unwrap().props[0].value else { panic!() };
    assert!(matches!(dict.entries[0], Entry::Laid(Value::GuidN(_), Value::Null, _)));
    assert!(matches!(dict.entries[1], Entry::Laid(Value::GuidN(_), Value::Int(1), _)));
    assert_eq!((s.dict_dropped, s.bp_uses.len()), (1, 2));
    // A constructor parameter is read through the parameter's type.
    let run = schema::ty(ty("Kingmaker.UnitLogic.Commands.UnitRunAway"));
    let target = run.members.iter().find(|m| m.json == "Target").unwrap();
    assert_eq!(target.codec, Codec::Vector3);
    assert!(target.flags & member_flags::CTOR_PARAM != 0);
    // Generic arguments that are enums are checked against their assembly too.
    assert!(arg_assembly_ok("System.Int32", Some("mscorlib"), "Assembly-CSharp"));
    assert!(!arg_assembly_ok("Kingmaker.Nope", Some("Assembly-CSharp"), "Assembly-CSharp"));
    let en = schema::enum_def(*e);
    assert!(arg_assembly_ok(en.clr, Some(en.assembly), "Assembly-CSharp"));
    assert!(!arg_assembly_ok(en.clr, Some("MissingAssembly"), "Assembly-CSharp"));
}

#[test]
fn root_types_by_entry_name() {
    assert_eq!(root_type("player.json"), Some("Kingmaker.Player"));
    assert_eq!(
        root_type("bad465e5404604346bdf71ef50b3299a.json"),
        Some("Kingmaker.EntitySystem.AreaPersistentState")
    );
    assert_eq!(
        root_type(
            "bad465e5404604346bdf71ef50b3299aPrologue_Caves_1_TerendelevScales_Mechanics.json"
        ),
        Some("Kingmaker.EntitySystem.SceneEntitiesState")
    );
    assert_eq!(root_type("history"), None);
    for n in [
        "header.json",
        "player.json",
        "party.json",
        "tbm.json",
        "0123456789abcdef0123456789abcdef.json",
    ] {
        assert!(root_index(n).is_some(), "{n}");
    }
}

#[test]
fn timespan_format() {
    for (t, s) in [
        (0, "00:00:00"),
        (434_213_070_000, "12:03:41.3070000"),
        (-10_000_000, "-00:00:01"),
        (937_845_000_000, "1.02:03:04.5000000"),
    ] {
        assert_eq!(fmt_timespan(t), s);
        assert_eq!(parse_timespan(s), Some(t));
    }
    assert_eq!(parse_timespan("1:2:3"), None);
}
