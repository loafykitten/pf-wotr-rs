//! The text template engine (decompiled, not copied: `Kingmaker.TextTools/
//! TextTemplateEngine.cs`, `TextTemplate.cs` and the templates it registers).
//!
//! `{tag}` and `{tag|p1|p2…}` are replaced by the registered template's output; anything
//! else is kept as written, an unknown tag included (`{0}` is a `string.Format` slot the
//! engine leaves alone). The scan is the C#'s, step for step; it works on UTF-16 code units
//! there and on bytes here, which agree because every delimiter is ASCII (where the C#
//! splits a string at its last code unit and appends the rest later, this appends it at
//! once). A tag is looked up lower-cased (`ToLowerInvariant`) and is *capitalized* when its
//! first character is upper case (`char.IsUpper`), which `race` uses.
//!
//! Templates that read game state ask the [`Env`]; each query defaults to
//! [`TextError::Unsupported`], so text that needs state a caller doesn't model fails closed
//! instead of coming out wrong. Templates the C# would throw in (`{}`, `{flag}` with no
//! parameter) are [`TextError::Throws`].

use std::fmt;

/// A text error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    /// The game throws here (it logs `Exception in string: …` and rethrows to its caller).
    Throws(String),
    /// A template needs state the caller's [`Env`] doesn't provide.
    Unsupported(String),
    /// The localization pack is damaged.
    Pack(String),
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Throws(s) => write!(f, "the game throws: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported text template state: {s}"),
            Self::Pack(s) => write!(f, "localization pack: {s}"),
        }
    }
}

impl std::error::Error for TextError {}

fn unsupported<T>(what: &str) -> Result<T, TextError> {
    Err(TextError::Unsupported(what.to_owned()))
}

/// `Gender`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male = 0,
    Female = 1,
}

/// `Color32` (alpha unused by the templates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color32 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color32 {
    /// `ColorUtilityEx.ToHtmlStringRGB(Color32)`: `RRGGBB`, upper-case hex.
    #[must_use]
    pub fn html_rgb(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Unity's `ColorUtility.ToHtmlStringRGB(Color)` on this colour converted to `Color`
    /// (each channel `c / 255f`), as the narrator template calls it: each channel back
    /// through `Mathf.RoundToInt(c * 255f)` (round half to even), clamped.
    #[must_use]
    pub fn html_rgb_via_color(self) -> String {
        let ch = |c: u8| {
            let f = f32::from(c) / 255.0 * 255.0;
            // In range by construction; the clamp is the C#'s.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let v = f64::from(f).round_ties_even().clamp(0.0, 255.0) as u8;
            v
        };
        Color32 { r: ch(self.r), g: ch(self.g), b: ch(self.b), a: 1 }.html_rgb()
    }
}

/// `TooltipType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipType {
    Glossary,
    Decisions,
    Mechanics,
}

/// A game clock reading, `TimeSpan`'s components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameTime {
    /// `TimeSpan.Ticks` (100 ns units).
    pub ticks: i64,
}

impl GameTime {
    const PER_SECOND: i64 = 10_000_000;

    /// `TimeSpan.Hours`: the hours component (−23 to 23).
    #[must_use]
    pub fn hours(self) -> i64 {
        self.ticks / (3600 * Self::PER_SECOND) % 24
    }

    /// `TimeSpan.Minutes`.
    #[must_use]
    pub fn minutes(self) -> i64 {
        self.ticks / (60 * Self::PER_SECOND) % 60
    }

    /// `TimeSpan.Seconds`.
    #[must_use]
    pub fn seconds(self) -> i64 {
        self.ticks / Self::PER_SECOND % 60
    }
}

/// .NET's `ToString("D2")` for an `int`: at least two digits, a minus sign before them.
fn d2(v: i64) -> String {
    if v < 0 { format!("-{:02}", v.unsigned_abs()) } else { format!("{v:02}") }
}

/// The game state templates read. Every query defaults to unsupported.
pub trait Env {
    /// `mf`: the gender of `GameLogContext.SourceUnit` when a log context is in scope, else
    /// the dialogue's acting unit's, else the main character's; `Male` when there's none.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn mf_gender(&mut self) -> Result<Gender, TextError> {
        unsupported("mf: the speaking unit's gender")
    }

    /// `Player.PlayerIsKing`: `BlueprintRoot.KingFlag` unlocked.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn player_is_king(&mut self) -> Result<bool, TextError> {
        unsupported("mf: Player.PlayerIsKing")
    }

    /// `name`: the dialogue's acting unit's `CharacterName`, else the main character's
    /// (already processed: a unit's name is itself a localized string).
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn name(&mut self) -> Result<String, TextError> {
        unsupported("name: the acting unit's name")
    }

    /// `race`: the main character's race's `Name`.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn race_name(&mut self) -> Result<String, TextError> {
        unsupported("race: the main character's race")
    }

    /// `n`: `BlueprintRoot.UIRoot.DialogColors.Narrator`.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn narrator_color(&mut self) -> Result<Color32, TextError> {
        unsupported("n: the narrator colour")
    }

    /// `g`, `d`, `m`: the tooltip colour of `ty`, from `UIRoot.BookEventColors` while the
    /// current dialogue is a book event and `UIRoot.DialogColors` otherwise.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn tooltip_color(&mut self, ty: TooltipType) -> Result<Color32, TextError> {
        let _ = ty;
        unsupported("g/d/m: the tooltip colours")
    }

    /// `flag`: `FlagStrings.GetFlagString(flag)`'s text (processed), `None` without one.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn flag_string(&mut self, flag: &str) -> Result<Option<String>, TextError> {
        let _ = flag;
        unsupported("flag: FlagStrings")
    }

    /// `time`, `date`: `TimeController.GameTime`.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn game_time(&mut self) -> Result<GameTime, TextError> {
        unsupported("time: the game time")
    }

    /// `date`: `BlueprintRoot.Calendar.GetDateText(gameTime, format)`.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn date_text(&mut self, time: GameTime, format: &str) -> Result<String, TextError> {
        let _ = (time, format);
        unsupported("date: the calendar")
    }

    /// `GameLogContext`'s unit field: `None` for null. `LogHelper.GetUnitName` writes
    /// `<no unit>` for a null one.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn log_unit(&mut self, field: LogUnitField) -> Result<Option<LogUnit>, TextError> {
        let _ = field;
        unsupported("the combat log's units (GameLogContext)")
    }

    /// `GameLogContext.SourceFact?.Name` (`None` for a null fact or a null name).
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn log_source_fact_name(&mut self) -> Result<Option<String>, TextError> {
        unsupported("GameLogContext.SourceFact")
    }

    /// `GameLogContext`'s string field (`None` for null).
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn log_str(&mut self, field: LogStrField) -> Result<Option<String>, TextError> {
        let _ = field;
        unsupported("the combat log's strings (GameLogContext)")
    }

    /// `GameLogContext`'s `int` field.
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn log_int(&mut self, field: LogIntField) -> Result<i32, TextError> {
        let _ = field;
        unsupported("the combat log's numbers (GameLogContext)")
    }

    /// `GameLogContext`'s roll field (`None` for null).
    ///
    /// # Errors
    ///
    /// When unsupported.
    fn log_roll(&mut self, field: LogRollField) -> Result<Option<LogRoll>, TextError> {
        let _ = field;
        unsupported("the combat log's rolls (GameLogContext)")
    }

    /// Any other template (the kingdom's, the UI's, tutorials'): its
    /// output, given the tag as registered (lower case).
    ///
    /// # Errors
    ///
    /// When unsupported (the default).
    fn other(
        &mut self,
        tag: &str,
        capitalized: bool,
        params: &[String],
    ) -> Result<String, TextError> {
        let _ = (capitalized, params);
        Err(TextError::Unsupported(format!("template {{{tag}}}")))
    }
}

/// An [`Env`] that supports nothing: text with templates fails closed.
#[derive(Debug, Default)]
pub struct NoEnv;

impl Env for NoEnv {}

/// The templates `TextTemplateEngine`'s static constructor registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Template {
    MaleFemale,
    Race,
    Name,
    Date,
    Time,
    Flag,
    NarratorStart,
    NarratorEnd,
    TooltipStart(TooltipType),
    TooltipEnd,
    /// `LeaderMaleFemaleTemplate`: always "".
    LeaderMaleFemale,
    LineBreak,
    Empty,
    /// `PcConsoleTemplate`: the first parameter.
    PcConsole,
    /// The combat log's templates (`LogTemplate*`), reading `GameLogContext`.
    Log(LogTag),
    Other,
}

/// The combat log's templates (`Kingmaker.TextTools/LogTemplate*.cs`), each reading one of
/// `GameLogContext`'s fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTag {
    Target,
    Formula,
    Source,
    Text,
    TextWithTags,
    Description,
    Count,
    CountForm,
    Roll,
    D20,
    D100,
    Modifier,
    Dc,
    ChanceDc,
    RollChance,
    AttackNumber,
    AttacksCount,
    Round,
}

/// A unit as `LogHelper.GetUnitName` shows it: `CharacterName` and
/// `ColorUtility.ToHtmlStringRGB(Blueprint.Color)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogUnit {
    pub name: String,
    pub color_rgb: String,
}

impl LogUnit {
    /// `LogHelper.GetUnitName`: `<b><color=#RRGGBB>name</color></b>`.
    #[must_use]
    pub fn rich(&self) -> String {
        format!("<b><color=#{}>{}</color></b>", self.color_rgb, self.name)
    }
}

/// `GameLogContext`'s unit fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogUnitField {
    SourceUnit,
    Target,
}

/// `GameLogContext`'s string fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStrField {
    Text,
    TextWithTags,
    Description,
    Formula,
}

/// `GameLogContext`'s `int` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogIntField {
    Count,
    Roll,
    Modifier,
    Dc,
    ChanceDc,
    RollChance,
    AttackNumber,
    AttacksCount,
    RoundNumber,
}

/// `GameLogContext`'s roll fields (`D20`, `D100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRollField {
    D20,
    D100,
}

/// A `RuleRollDice` as `LogHelper.GetRollDescription` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRoll {
    /// `Result`.
    pub result: i32,
    /// `RollHistory` (`None`: null).
    pub history: Option<Vec<i32>>,
    /// `ReplacedOne`.
    pub replaced_one: bool,
    /// `ReplaceOneWithMax`.
    pub replace_one_with_max: bool,
    /// `Rerolls`: each reroll's source blueprint's `Name` when it is a `BlueprintUnitFact`
    /// (`None` for another source); `None` for a null list.
    pub rerolls: Option<Vec<Option<String>>>,
}

impl LogRoll {
    /// `LogHelper.GetRollDescription`.
    #[must_use]
    pub fn description(&self) -> String {
        let history = match &self.history {
            Some(h) if h.len() != 1 => h,
            _ => {
                return if self.replaced_one {
                    "<s>1</s> 20".to_owned()
                } else {
                    self.result.to_string()
                };
            }
        };
        let mut s = String::new();
        let mut first = true;
        let mut marked = false;
        for &item in history {
            let (open, close) = if item == self.result && !marked {
                marked = true;
                ("<b><u>", "</u></b>")
            } else {
                ("", "")
            };
            if !first {
                s.push_str(", ");
            }
            first = false;
            if self.replace_one_with_max && item == 1 {
                s.push_str(open);
                s.push_str("<s>1</s> 20");
                s.push_str(close);
            } else {
                s.push_str(open);
                s.push_str(&item.to_string());
                s.push_str(close);
            }
        }
        if let Some(r) = &self.rerolls {
            let text: String = r.iter().flatten().map(String::as_str).collect();
            if !text.is_empty() {
                s.push_str(" [");
                s.push_str(&text);
                s.push(']');
            }
        }
        s
    }
}

/// The registered tags, lower case (`AddTemplate` lower-cases them).
const TAGS: &[(&str, Template)] = &[
    ("mf", Template::MaleFemale),
    ("race", Template::Race),
    ("name", Template::Name),
    ("kingdomname", Template::Other),
    ("date", Template::Date),
    ("time", Template::Time),
    ("custom_companion_cost", Template::Other),
    ("respec_cost", Template::Other),
    ("leader_cost", Template::Other),
    ("flag", Template::Flag),
    ("n", Template::NarratorStart),
    ("/n", Template::NarratorEnd),
    ("g", Template::TooltipStart(TooltipType::Glossary)),
    ("/g", Template::TooltipEnd),
    ("d", Template::TooltipStart(TooltipType::Decisions)),
    ("/d", Template::TooltipEnd),
    ("m", Template::TooltipStart(TooltipType::Mechanics)),
    ("/m", Template::TooltipEnd),
    ("mf_regent", Template::LeaderMaleFemale),
    ("mf_counsilor", Template::LeaderMaleFemale),
    ("mf_granddiplomat", Template::LeaderMaleFemale),
    ("mf_magister", Template::LeaderMaleFemale),
    ("mf_general", Template::LeaderMaleFemale),
    ("target", Template::Log(LogTag::Target)),
    ("formula", Template::Log(LogTag::Formula)),
    ("source", Template::Log(LogTag::Source)),
    ("text", Template::Log(LogTag::Text)),
    ("text_with_tags", Template::Log(LogTag::TextWithTags)),
    ("description", Template::Log(LogTag::Description)),
    ("count", Template::Log(LogTag::Count)),
    ("count_form", Template::Log(LogTag::CountForm)),
    ("roll", Template::Log(LogTag::Roll)),
    ("d20", Template::Log(LogTag::D20)),
    ("d100", Template::Log(LogTag::D100)),
    ("mod", Template::Log(LogTag::Modifier)),
    ("dc", Template::Log(LogTag::Dc)),
    ("chance_dc", Template::Log(LogTag::ChanceDc)),
    ("roll_chance", Template::Log(LogTag::RollChance)),
    ("slotname", Template::Other),
    ("wielder", Template::Other),
    ("rations", Template::Other),
    ("recipe", Template::Other),
    ("attack_number", Template::Log(LogTag::AttackNumber)),
    ("attacks_count", Template::Log(LogTag::AttacksCount)),
    ("round", Template::Log(LogTag::Round)),
    ("claimed_villages_count", Template::Other),
    ("claimed_towns_count", Template::Other),
    ("claimed_cities_count", Template::Other),
    ("reserve_count", Template::Other),
    ("portraits_path", Template::Other),
    ("area_name", Template::Other),
    ("bind", Template::Other),
    ("console_bind", Template::Other),
    ("empty", Template::Empty),
    ("br", Template::LineBreak),
    ("app_path", Template::Other),
    ("pc_console", Template::PcConsole),
    ("t", Template::Other),
    ("ui", Template::Other),
];

/// The registered tags (`TextTemplateEngine.GetTemplateTags`, in registration order).
#[must_use]
pub fn template_tags() -> Vec<&'static str> {
    TAGS.iter().map(|(t, _)| *t).collect()
}

/// `string.ToLowerInvariant` on the game's runtime (Mono): each UTF-16 code unit through
/// [`crate::casing::LOWER`]; characters outside the BMP (surrogate pairs) are unchanged.
#[must_use]
pub fn to_lower_invariant(s: &str) -> String {
    s.chars()
        .map(|c| {
            u16::try_from(u32::from(c))
                .ok()
                .and_then(|u| {
                    crate::casing::LOWER
                        .binary_search_by_key(&u, |p| p.0)
                        .ok()
                        .map(|i| crate::casing::LOWER[i].1)
                })
                .and_then(|l| char::from_u32(u32::from(l)))
                .unwrap_or(c)
        })
        .collect()
}

fn lookup(tag: &str) -> Option<(&'static str, Template)> {
    let lower = to_lower_invariant(tag);
    TAGS.iter().find(|(t, _)| *t == lower).copied()
}

fn generate(
    tag: &str,
    t: Template,
    capitalized: bool,
    params: &[String],
    env: &mut dyn Env,
) -> Result<String, TextError> {
    Ok(match t {
        Template::MaleFemale => {
            let mut i = env.mf_gender()? as usize;
            if params.len() > 2 && env.player_is_king()? {
                i += 2;
            }
            params.get(i).cloned().unwrap_or_default()
        }
        Template::Race => {
            let name = env.race_name()?;
            if capitalized { name } else { to_lower_invariant(&name) }
        }
        Template::Name => env.name()?,
        Template::Date => {
            // The C# logs a missing parameter, then indexes it anyway.
            let Some(format) = params.first() else {
                return Err(TextError::Throws("{date}: parameter is missing".into()));
            };
            let time = env.game_time()?;
            env.date_text(time, format)?
        }
        Template::Time => {
            let format = params.first().filter(|f| !f.is_empty()).map_or("HH:MM", String::as_str);
            let time = env.game_time()?;
            format
                .replace("HH", &d2(time.hours()))
                .replace("MM", &d2(time.minutes()))
                .replace("SS", &d2(time.seconds()))
        }
        Template::Flag => {
            let Some(flag) = params.first() else {
                return Err(TextError::Throws("{flag}: no parameter".into()));
            };
            env.flag_string(flag)?.unwrap_or_default()
        }
        Template::NarratorStart => {
            format!("<color=#{}>", env.narrator_color()?.html_rgb_via_color())
        }
        Template::NarratorEnd => "</color>".to_owned(),
        Template::TooltipStart(ty) => {
            let link = params.first().map_or("", String::as_str);
            format!("<b><color=#{}><link=\"{link}\">", env.tooltip_color(ty)?.html_rgb())
        }
        Template::TooltipEnd => "</link></color></b>".to_owned(),
        Template::LeaderMaleFemale | Template::Empty => String::new(),
        Template::LineBreak => "\n".to_owned(),
        Template::PcConsole => params.first().cloned().unwrap_or_default(),
        Template::Log(l) => log_template(l, params, env)?,
        Template::Other => env.other(tag, capitalized, params)?,
    })
}

/// The combat log's templates (`LogTemplate*.Generate`, `LogHelper`): a null string appends
/// nothing (`StringBuilder.Append(null)`).
fn log_template(l: LogTag, params: &[String], env: &mut dyn Env) -> Result<String, TextError> {
    let unit = |env: &mut dyn Env, f| -> Result<String, TextError> {
        Ok(env.log_unit(f)?.map_or_else(|| "<no unit>".to_owned(), |u| u.rich()))
    };
    let int =
        |env: &mut dyn Env, f| -> Result<String, TextError> { Ok(env.log_int(f)?.to_string()) };
    let roll = |env: &mut dyn Env, f| -> Result<String, TextError> {
        // A null roll: `LogHelper` logs an error and writes "".
        Ok(env.log_roll(f)?.map(|r| r.description()).unwrap_or_default())
    };
    Ok(match l {
        LogTag::Target => unit(env, LogUnitField::Target)?,
        LogTag::Source => match env.log_unit(LogUnitField::SourceUnit)? {
            Some(u) => u.rich(),
            None => env.log_source_fact_name()?.unwrap_or_default(),
        },
        LogTag::Text => env.log_str(LogStrField::Text)?.unwrap_or_default(),
        LogTag::Description => env.log_str(LogStrField::Description)?.unwrap_or_default(),
        LogTag::Formula => env.log_str(LogStrField::Formula)?.unwrap_or_default(),
        LogTag::TextWithTags => match env.log_str(LogStrField::TextWithTags)? {
            Some(t) => process(&t, env)?,
            None => return Err(TextError::Throws("{text_with_tags}: Process(null)".into())),
        },
        LogTag::Count => int(env, LogIntField::Count)?,
        LogTag::CountForm => {
            let i = usize::from(env.log_int(LogIntField::Count)? != 1);
            params.get(i).cloned().unwrap_or_default()
        }
        LogTag::Roll => int(env, LogIntField::Roll)?,
        LogTag::Modifier => int(env, LogIntField::Modifier)?,
        LogTag::Dc => int(env, LogIntField::Dc)?,
        LogTag::ChanceDc => int(env, LogIntField::ChanceDc)?,
        LogTag::RollChance => int(env, LogIntField::RollChance)?,
        LogTag::AttacksCount => int(env, LogIntField::AttacksCount)?,
        LogTag::Round => int(env, LogIntField::RoundNumber)?,
        LogTag::AttackNumber => {
            // `((int)GameLogContext.AttackNumber + 1).ToString()`, unchecked.
            env.log_int(LogIntField::AttackNumber)?.wrapping_add(1).to_string()
        }
        LogTag::D20 => roll(env, LogRollField::D20)?,
        LogTag::D100 => roll(env, LogRollField::D100)?,
    })
}

/// `TextTemplateEngine.Process(text)`.
///
/// # Errors
///
/// [`TextError::Throws`] where the game throws, [`TextError::Unsupported`] where a
/// template needs state `env` doesn't provide.
pub fn process(text: &str, env: &mut dyn Env) -> Result<String, TextError> {
    scan(text, &mut |name, t, capitalized, params| generate(name, t, capitalized, params, env))
}

/// A template call the scan found: the registered tag, the template, capitalized, the
/// parameters.
type Call<'a> =
    dyn FnMut(&'static str, Template, bool, &[String]) -> Result<String, TextError> + 'a;

/// The scan, with each template call made through `call`.
fn scan(text: &str, call: &mut Call<'_>) -> Result<String, TextError> {
    if !text.contains('{') {
        return Ok(text.to_owned());
    }
    let b = text.as_bytes();
    let len = b.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;
    while i < len {
        if b[i] != b'{' {
            let j = text[i..].find('{').map_or(len, |k| i + k);
            out.push_str(&text[i..j]);
            i = j;
            continue;
        }
        let start = i;
        i += 1;
        while i < len && !matches!(b[i], b'}' | b'|' | b'{') {
            i += 1;
        }
        if i >= len || b[i] == b'{' {
            out.push_str(&text[start..i]);
            continue;
        }
        let tag = &text[start + 1..i];
        // `char.IsUpper(text2[0])` throws on an empty tag. A tag only matches when it is ASCII
        // (no other code unit lower-cases to ASCII on Mono), so `IsUpper` is ASCII's there.
        let Some(first) = tag.chars().next() else {
            return Err(TextError::Throws(format!("empty template tag in: {text}")));
        };
        let capitalized = first.is_ascii_uppercase();
        let Some((name, t)) = lookup(tag) else {
            while i < len && b[i] != b'}' {
                i += 1;
            }
            out.push_str(&text[start..i]);
            continue;
        };
        let mut params = Vec::new();
        if b[i] == b'}' {
            out.push_str(&call(name, t, capitalized, &params)?);
            i += 1;
            continue;
        }
        // At the first `|`: the parameters up to `}`, unless a `{` or the end comes first.
        let mut from = i;
        let mut j = i;
        let closed = loop {
            j += 1;
            if j >= len {
                break None;
            }
            if matches!(b[j], b'|' | b'}') {
                params.push(text[from + 1..j].to_owned());
                from = j;
            }
            match b[j] {
                b'}' => break Some(true),
                b'{' => break Some(false),
                _ => {}
            }
        };
        match closed {
            Some(true) => {
                out.push_str(&call(name, t, capitalized, &params)?);
                i = j + 1;
            }
            Some(false) => {
                out.push_str(&text[start..j]);
                i = j;
            }
            None => {
                out.push_str(&text[start..]);
                i = len;
            }
        }
    }
    Ok(out)
}

/// The scan with every registered template replaced by `<tag:capitalized:p1,p2…>`, for
/// checking the scan against the C#'s on the same stand-in output.
///
/// # Errors
///
/// Where the game throws.
#[doc(hidden)]
pub fn scan_generic(text: &str) -> Result<String, TextError> {
    scan(text, &mut |name, _, capitalized, params| {
        Ok(format!("<{name}:{capitalized}:{}>", params.join(",")))
    })
}
