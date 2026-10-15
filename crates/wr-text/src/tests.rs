use std::collections::BTreeMap;

use super::*;

/// A test environment with every query answered.
struct Full {
    gender: Gender,
    king: bool,
    book: bool,
}

impl Default for Full {
    fn default() -> Self {
        Self { gender: Gender::Male, king: false, book: false }
    }
}

impl Env for Full {
    fn mf_gender(&mut self) -> Result<Gender, TextError> {
        Ok(self.gender)
    }
    fn player_is_king(&mut self) -> Result<bool, TextError> {
        Ok(self.king)
    }
    fn name(&mut self) -> Result<String, TextError> {
        Ok("Rowan".into())
    }
    fn race_name(&mut self) -> Result<String, TextError> {
        Ok("Half-Elf".into())
    }
    fn narrator_color(&mut self) -> Result<Color32, TextError> {
        Ok(Color32 { r: 0x8a, g: 0x6d, b: 0x3b, a: 255 })
    }
    fn tooltip_color(&mut self, ty: TooltipType) -> Result<Color32, TextError> {
        let v = if self.book { 0x10 } else { 0 } + ty as u8;
        Ok(Color32 { r: v, g: 0xab, b: 0xcd, a: 255 })
    }
    fn flag_string(&mut self, flag: &str) -> Result<Option<String>, TextError> {
        Ok((flag == "known").then(|| "a flag".to_owned()))
    }
    fn game_time(&mut self) -> Result<GameTime, TextError> {
        // 3 days, 7:05:09.
        Ok(GameTime { ticks: ((3 * 24 + 7) * 3600 + 5 * 60 + 9) * 10_000_000 })
    }
    fn other(
        &mut self,
        tag: &str,
        capitalized: bool,
        params: &[String],
    ) -> Result<String, TextError> {
        Ok(format!("<{tag}:{capitalized}:{}>", params.join(",")))
    }
}

fn p(text: &str) -> Result<String, TextError> {
    process(text, &mut Full::default())
}

#[test]
fn plain_text_is_unchanged() {
    assert_eq!(p("no templates | here }").unwrap(), "no templates | here }");
    assert_eq!(p("").unwrap(), "");
}

#[test]
fn glossary_and_narrator() {
    assert_eq!(
        p("{g|Encyclopedia:DC}DC{/g} 16").unwrap(),
        "<b><color=#00ABCD><link=\"Encyclopedia:DC\">DC</link></color></b> 16"
    );
    assert_eq!(p("{n}Quiet.{/n}").unwrap(), "<color=#8A6D3B>Quiet.</color>");
    assert_eq!(
        process("{d|x}{m|y}", &mut Full { book: true, ..Full::default() }).unwrap(),
        "<b><color=#11ABCD><link=\"x\"><b><color=#12ABCD><link=\"y\">"
    );
}

#[test]
fn male_female_and_king() {
    assert_eq!(p("{mf|he|she}").unwrap(), "he");
    let mut f = Full { gender: Gender::Female, ..Full::default() };
    assert_eq!(process("{mf|he|she}", &mut f).unwrap(), "she");
    assert_eq!(process("{mf|he|she|king|queen}", &mut f).unwrap(), "she");
    f.king = true;
    assert_eq!(process("{mf|he|she|king|queen}", &mut f).unwrap(), "queen");
    // Two parameters: the king flag isn't read and doesn't shift.
    assert_eq!(process("{mf|he|she}", &mut f).unwrap(), "she");
    // Missing entries come out empty.
    assert_eq!(process("{mf|he}", &mut f).unwrap(), "");
    assert_eq!(p("{mf}").unwrap(), "");
    assert_eq!(p("{MF|a|b}").unwrap(), "a");
}

#[test]
fn race_capitalization() {
    assert_eq!(p("a {race}").unwrap(), "a half-elf");
    assert_eq!(p("{Race}").unwrap(), "Half-Elf");
    assert_eq!(p("{name}, {Name}").unwrap(), "Rowan, Rowan");
}

#[test]
fn unknown_tags_are_kept() {
    assert_eq!(p("{0} and {1}").unwrap(), "{0} and {1}");
    assert_eq!(p("{foo|a|b} x").unwrap(), "{foo|a|b} x");
    assert_eq!(p("{foo|a").unwrap(), "{foo|a");
}

#[test]
fn unterminated_and_nested_braces() {
    assert_eq!(p("abc {mf|x").unwrap(), "abc {mf|x");
    assert_eq!(p("{mf").unwrap(), "{mf");
    assert_eq!(p("{").unwrap(), "{");
    assert_eq!(p("x{").unwrap(), "x{");
    assert_eq!(p("{mf|").unwrap(), "{mf|");
    assert_eq!(p("{{n}").unwrap(), "{<color=#8A6D3B>");
    assert_eq!(p("{mf|a{n}b").unwrap(), "{mf|a<color=#8A6D3B>b");
    assert_eq!(p("{mf|é").unwrap(), "{mf|é");
    assert_eq!(p("{mf|é|ü}").unwrap(), "é");
    assert_eq!(p("ü{n}é{/n}").unwrap(), "ü<color=#8A6D3B>é</color>");
}

#[test]
fn empty_tag_throws() {
    assert!(matches!(p("{}"), Err(TextError::Throws(_))));
    assert!(matches!(p("x {|a}"), Err(TextError::Throws(_))));
    assert!(matches!(p("{flag}"), Err(TextError::Throws(_))));
    assert!(matches!(p("{date}"), Err(TextError::Throws(_))));
}

#[test]
fn stateless_templates() {
    assert_eq!(p("a{br}b{empty}c").unwrap(), "a\nbc");
    assert_eq!(p("{pc_console|click|press}").unwrap(), "click");
    assert_eq!(p("{mf_Regent|a|b}").unwrap(), "");
    assert_eq!(p("{flag|known}/{flag|other}").unwrap(), "a flag/");
}

#[test]
fn time() {
    assert_eq!(p("{time}").unwrap(), "07:05");
    assert_eq!(p("{time|HH:MM:SS}").unwrap(), "07:05:09");
    assert_eq!(p("{time|}").unwrap(), "07:05");
}

#[test]
fn other_templates_go_to_the_env() {
    // The kingdom's, the UI's (the combat log's are `log_templates`).
    assert_eq!(p("{Bind|x|y} {kingdomname}").unwrap(), "<bind:true:x,y> <kingdomname:false:>");
}

#[test]
fn no_env_fails_closed() {
    assert!(matches!(process("{n}", &mut NoEnv), Err(TextError::Unsupported(_))));
    assert!(matches!(process("{source}", &mut NoEnv), Err(TextError::Unsupported(_))));
    // Templates that need no state don't ask.
    assert_eq!(process("{br}{/g}{0}", &mut NoEnv).unwrap(), "\n</link></color></b>{0}");
}

#[test]
fn colours_round_trip_through_color() {
    for c in 0..=255u8 {
        let col = Color32 { r: c, g: 255 - c, b: c / 2, a: 255 };
        assert_eq!(col.html_rgb_via_color(), col.html_rgb());
    }
}

#[test]
fn lower_invariant() {
    // Mono's invariant casing: U+0130 and the Kelvin sign are kept, Σ isn't contextual.
    assert_eq!(engine::to_lower_invariant("ÀB\u{130}ΣΣ\u{212A}😀"), "àb\u{130}σσ\u{212A}😀");
    assert_eq!(p("{T\u{130}ME}").unwrap(), "{T\u{130}ME}");
    assert_eq!(p("{\u{212A}ingdomname}").unwrap(), "{\u{212A}ingdomname}");
    for &(c, l) in crate::casing::LOWER {
        assert!(char::from_u32(u32::from(c)).is_some() && char::from_u32(u32::from(l)).is_some());
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("wr-text-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn pack_round_trip() {
    let mut s = BTreeMap::new();
    s.insert("b".to_owned(), "bee {n}".to_owned());
    s.insert("a".to_owned(), String::new());
    s.insert("ü".to_owned(), "umlaut".to_owned());
    let path = tmp("rt.wrtext");
    pack::write(&s, (1, 2), &path).unwrap();
    // SAFETY: a file this test alone writes.
    let pk = unsafe { Pack::open(&path, Locale::EnGb) }.unwrap();
    assert_eq!(pk.len(), 3);
    assert_eq!(pk.find("a").unwrap(), Some(""));
    assert_eq!(pk.find("ü").unwrap(), Some("umlaut"));
    assert_eq!(pk.find("c").unwrap(), None);
    assert_eq!(pk.get_text("c", true).unwrap(), "[unknown key: c]");
    assert_eq!(pk.get_text("c", false).unwrap(), "");
    assert_eq!(display(Some(&pk), "", &mut NoEnv).unwrap(), "");
    assert_eq!(display(None, "b", &mut NoEnv).unwrap(), "");
    assert_eq!(display(Some(&pk), "b", &mut Full::default()).unwrap(), "bee <color=#8A6D3B>");
    assert!(display(Some(&pk), "b", &mut NoEnv).is_err());
    let keys: Vec<_> = pk.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["a", "b", "ü"]);
}

#[test]
fn damaged_packs_are_rejected() {
    let mut s = BTreeMap::new();
    s.insert("a".to_owned(), "x".to_owned());
    s.insert("b".to_owned(), "y".to_owned());
    let path = tmp("dmg.wrtext");
    pack::write(&s, (0, 0), &path).unwrap();
    let good = std::fs::read(&path).unwrap();
    let open = |bytes: &[u8]| {
        let p = tmp("dmg2.wrtext");
        std::fs::write(&p, bytes).unwrap();
        // SAFETY: a file this test alone writes.
        unsafe { Pack::open(&p, Locale::EnGb) }
    };
    assert!(open(&good).is_ok());
    assert!(open(&good[..good.len() - 1]).is_err(), "truncated blob");
    assert!(open(&good[..30]).is_err(), "truncated index");
    let mut bad = good.clone();
    bad[0] = b'X';
    assert!(open(&bad).is_err(), "magic");
    // Swap the two entries: keys out of order.
    let mut swapped = good.clone();
    let (h, e) = (28, 16);
    let (first, second) = (good[h..h + e].to_vec(), good[h + e..h + 2 * e].to_vec());
    swapped[h..h + e].copy_from_slice(&second);
    swapped[h + e..h + 2 * e].copy_from_slice(&first);
    assert!(open(&swapped).is_err(), "order");
    // A key byte that isn't UTF-8 (the blob starts with the first key, "a").
    let mut bad_key = good.clone();
    bad_key[28 + 2 * 16] = 0xFF;
    assert!(open(&bad_key).is_err(), "key UTF-8");
}

#[test]
fn json_pack_reading() {
    let path = tmp("p.json");
    std::fs::write(&path, r#"{"$id":"1","strings":{"k":"v","n":null,"k":"w"}}"#).unwrap();
    let s = pack::read_json(&path).unwrap();
    assert_eq!(s.get("k").map(String::as_str), Some("w"));
    assert_eq!(s.get("n").map(String::as_str), Some(""));
}

#[test]
fn concurrent_writers_each_replace_the_pack_whole() {
    let mut s = BTreeMap::new();
    for i in 0..2000 {
        s.insert(format!("k{i:05}"), format!("text {i}"));
    }
    let path = tmp("race.wrtext");
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| pack::write(&s, (3, 4), &path).unwrap());
        }
    });
    // SAFETY: the writers are done; nothing else writes this file.
    let pk = unsafe { Pack::open(&path, Locale::EnGb) }.unwrap();
    assert_eq!(pk.len(), 2000);
    assert_eq!(pk.find("k01234").unwrap(), Some("text 1234"));
    let leftovers = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("race.wrtext."))
        .count();
    assert_eq!(leftovers, 0);
}

/// A combat log context (`GameLogContext`) for the log templates.
#[derive(Default)]
struct Log {
    source: Option<LogUnit>,
    target: Option<LogUnit>,
    fact: Option<String>,
    text: Option<String>,
    tags: Option<String>,
    count: i32,
    attack_number: i32,
    d20: Option<LogRoll>,
}

impl Env for Log {
    fn log_unit(&mut self, field: LogUnitField) -> Result<Option<LogUnit>, TextError> {
        Ok(match field {
            LogUnitField::SourceUnit => self.source.clone(),
            LogUnitField::Target => self.target.clone(),
        })
    }
    fn log_source_fact_name(&mut self) -> Result<Option<String>, TextError> {
        Ok(self.fact.clone())
    }
    fn log_str(&mut self, field: LogStrField) -> Result<Option<String>, TextError> {
        Ok(match field {
            LogStrField::Text => self.text.clone(),
            LogStrField::TextWithTags => self.tags.clone(),
            _ => None,
        })
    }
    fn log_int(&mut self, field: LogIntField) -> Result<i32, TextError> {
        Ok(match field {
            LogIntField::Count => self.count,
            LogIntField::AttackNumber => self.attack_number,
            _ => 0,
        })
    }
    fn log_roll(&mut self, field: LogRollField) -> Result<Option<LogRoll>, TextError> {
        Ok(match field {
            LogRollField::D20 => self.d20.clone(),
            LogRollField::D100 => None,
        })
    }
}

#[test]
fn log_templates() {
    let unit = |n: &str| LogUnit { name: n.into(), color_rgb: "262626".into() };
    let mut e = Log { source: Some(unit("A")), count: 1, attack_number: 0, ..Log::default() };
    // `LogHelper.GetUnitName`, and a null unit.
    assert_eq!(
        process("{source} hits {target}", &mut e).unwrap(),
        "<b><color=#262626>A</color></b> hits <no unit>"
    );
    // `{source}` without a unit: the fact's name, else "".
    e.source = None;
    assert_eq!(process("[{source}]", &mut e).unwrap(), "[]");
    e.fact = Some("Bless".into());
    assert_eq!(process("[{Source}]", &mut e).unwrap(), "[Bless]");
    // `count_form`: the first form for exactly one.
    assert_eq!(process("{count} {count_form|point|points}", &mut e).unwrap(), "1 point");
    e.count = 2;
    assert_eq!(process("{count} {count_form|point|points}", &mut e).unwrap(), "2 points");
    // `attack_number` is one-based; a null string appends nothing.
    assert_eq!(process("#{attack_number}{text}", &mut e).unwrap(), "#1");
    e.attack_number = i32::MAX;
    assert_eq!(process("{attack_number}", &mut e).unwrap(), i32::MIN.to_string());
    // `text_with_tags` is processed again; null throws (`Process(null)`).
    e.tags = Some("{count}x".into());
    assert_eq!(process("{text_with_tags}", &mut e).unwrap(), "2x");
    e.tags = None;
    assert!(matches!(process("{text_with_tags}", &mut e), Err(TextError::Throws(_))));
    // A null roll writes "".
    assert_eq!(process("({d20})", &mut e).unwrap(), "()");
    // Unsupported by default.
    assert!(matches!(process("{count}", &mut NoEnv), Err(TextError::Unsupported(_))));
}

#[test]
fn roll_descriptions() {
    let r = |result, history: Option<Vec<i32>>| LogRoll {
        result,
        history,
        replaced_one: false,
        replace_one_with_max: false,
        rerolls: None,
    };
    assert_eq!(r(7, None).description(), "7");
    assert_eq!(r(7, Some(vec![7])).description(), "7");
    // One kept roll, marked once (the first equal one).
    assert_eq!(r(15, Some(vec![3, 15, 15])).description(), "3, <b><u>15</u></b>, 15");
    let mut one = r(20, Some(vec![1]));
    one.replaced_one = true;
    assert_eq!(one.description(), "<s>1</s> 20");
    let mut max = r(20, Some(vec![1, 4]));
    max.replace_one_with_max = true;
    max.rerolls = Some(vec![Some("Luck".into()), None, Some("Fate".into())]);
    assert_eq!(max.description(), "<s>1</s> 20, 4 [LuckFate]");
}
