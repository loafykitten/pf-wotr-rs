//! Text as the game shows it: `LocalizedString`s turned into strings through `wr-text`'s pack
//! and template engine, with the game state the templates read taken from the world
//! (decompiled, not copied: `Kingmaker.TextTools/{MaleFemaleTemplate,NameTemplate,
//! RaceTemplate,NarratorStartTemplate,TooltipStartTemplate}.cs`,
//! `Kingmaker.UnitLogic/UnitDescriptor.cs` (`CharacterName`, `Gender`),
//! `Kingmaker.Blueprints/BlueprintUnit.cs` (`CharacterName`), `Kingmaker/Player.cs`
//! (`PlayerIsKing`)).
//!
//! `GameLogContext` is never in scope where the sim reads text (the dialogue controller's
//! answers and ticks), so `mf` reads the acting unit or the main character. Templates the
//! world doesn't model (the combat log's, the kingdom's, the UI's, `flag`, `date`, `time`)
//! fail closed.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use wr_content::{Guid, Node};
use wr_text::{Color32, Env, Gender, Pack, TextError, TooltipType};

use crate::content::View;
use crate::world::{EntityIdx, World};

/// The keys of the `SharedStringAsset`s in `BlueprintReferencedAssets`, by their index (the
/// runtime pack's `{"asset_index": i}`), as `tools/wr-rules-data` extracts them from
/// `blueprint.assets` into `cache/sim/shared-strings.json`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SharedStrings {
    pub schema: String,
    pub source: BTreeMap<String, serde_json::Value>,
    pub keys: BTreeMap<i32, String>,
}

pub const SHARED_STRINGS_SCHEMA: &str = "wr-shared-strings-v1";

impl SharedStrings {
    /// `cache/sim/shared-strings.json`.
    #[must_use]
    pub fn default_path() -> std::path::PathBuf {
        wr_paths::cache_root().join("sim/shared-strings.json")
    }

    /// Loads the extracted table.
    ///
    /// # Errors
    ///
    /// If it's missing (run `wr-rules-data`), unreadable or another schema.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let b = std::fs::read(path)
            .with_context(|| format!("{} (run wr-rules-data)", path.display()))?;
        let s: Self = serde_json::from_slice(&b).context("shared-strings.json")?;
        anyhow::ensure!(
            s.schema == SHARED_STRINGS_SCHEMA,
            "shared-strings.json: schema {}",
            s.schema
        );
        Ok(s)
    }
}

/// What text is shown from: the localization pack (`LocalizationManager.CurrentPack`) and the
/// shared strings' keys.
#[derive(Debug)]
pub struct TextData {
    pub pack: Pack,
    pub shared: SharedStrings,
}

impl TextData {
    /// The English pack and the extracted shared strings.
    ///
    /// # Errors
    ///
    /// If either is missing.
    pub fn load_default() -> Result<Self> {
        Ok(Self {
            pack: Pack::load(wr_text::Locale::EnGb)?,
            shared: SharedStrings::load(&SharedStrings::default_path())?,
        })
    }
}

/// The template environment over a world.
#[derive(Debug)]
pub struct WorldEnv<'a> {
    pub w: &'a World,
}

fn data(w: &World) -> Result<&TextData, TextError> {
    w.text.as_deref().ok_or_else(|| TextError::Unsupported("no localization pack loaded".into()))
}

fn pack(w: &World) -> Result<&Pack, TextError> {
    Ok(&data(w)?.pack)
}

/// A `LocalizedString` with actual key `key`, as a string.
///
/// # Errors
///
/// When the text needs state the world doesn't model, or no pack is loaded.
pub fn localized(w: &World, key: &str) -> Result<String, TextError> {
    // `LoadString`: an empty key is "" whatever the pack.
    if key.is_empty() {
        return Ok(String::new());
    }
    wr_text::display(Some(pack(w)?), key, &mut WorldEnv { w })
}

/// A `LocalizedString` field (`m_Key`, the pack's actual key) as a string; a null field is
/// the implicit conversion's `"<null>"`.
///
/// # Errors
///
/// As [`localized`].
pub fn localized_field(w: &World, v: View<'_>, field: &str) -> Result<String, TextError> {
    match v.obj(field) {
        Some(s) => localized(w, s.str("m_Key").unwrap_or("")),
        None => Ok("<null>".to_owned()),
    }
}

/// A `SharedStringAsset` field's string's actual key, `None` for a null asset: from the
/// runtime pack, its referenced-assets index resolved through the extracted table; from the
/// zip, the key the converter reads.
///
/// # Errors
///
/// An index the table doesn't have (not a shared string, or no table loaded).
pub fn shared_string_key(w: &World, v: View<'_>, field: &str) -> Result<Option<String>, TextError> {
    match v.node(field) {
        Some(Node::SharedString(Some(r))) => Ok(Some(r.key.to_string())),
        Some(Node::AssetIndex(i)) if *i >= 0 => {
            data(w)?.shared.keys.get(i).map(|k| Some(k.clone())).ok_or_else(|| {
                TextError::Unsupported(format!(
                    "{field}: referenced asset {i} isn't a shared string"
                ))
            })
        }
        _ => Ok(None),
    }
}

/// `BlueprintUnit.CharacterName`: its `LocalizedName`'s string, `-unit name not set-`
/// without one.
///
/// # Errors
///
/// As [`localized`], and for a missing blueprint.
pub fn blueprint_character_name(w: &World, bp: Guid) -> Result<String, TextError> {
    let b = w
        .content
        .get(bp)
        .ok_or_else(|| TextError::Unsupported(format!("unit blueprint {bp} missing")))?;
    match shared_string_key(w, b.view(), "LocalizedName")? {
        Some(k) => localized(w, &k),
        None => Ok("-unit name not set-".to_owned()),
    }
}

/// `UnitDescriptor.CharacterName`: `ReplaceBlueprintForInspection`'s name, else the custom
/// name, else the blueprint's.
///
/// # Errors
///
/// As [`blueprint_character_name`].
pub fn character_name(w: &World, u: EntityIdx) -> Result<String, TextError> {
    let unit = w.unit(u).ok_or_else(|| TextError::Unsupported("not a unit".into()))?;
    if let Some(r) = unit.replace_blueprint_for_inspection {
        return blueprint_character_name(w, r);
    }
    if !unit.custom_name.is_empty() {
        return Ok(unit.custom_name.clone());
    }
    let bp =
        unit.blueprint.ok_or_else(|| TextError::Unsupported("unit without blueprint".into()))?;
    blueprint_character_name(w, bp)
}

/// `UnitDescriptor.Gender`: `CustomGender ?? Blueprint.Gender`.
#[must_use]
pub fn gender(w: &World, u: EntityIdx) -> Gender {
    let unit = w.unit(u);
    let g = unit.and_then(|x| x.custom_gender).unwrap_or_else(|| {
        unit.and_then(|x| x.blueprint)
            .and_then(|g| w.content.get(g))
            .map_or(0, |b| b.view().int("Gender"))
    });
    if g == 1 { Gender::Female } else { Gender::Male }
}

/// A packed `Color32` field (`r` in the low byte).
fn color32(v: View<'_>, field: &str) -> Result<Color32, TextError> {
    match v.node(field) {
        Some(Node::Int(i)) => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let [r, g, b, a] = (*i as i32 as u32).to_le_bytes();
            Ok(Color32 { r, g, b, a })
        }
        _ => Err(TextError::Unsupported(format!("colour {field} missing"))),
    }
}

/// Unity's `ColorUtility.ToHtmlStringRGB(Color)`: each channel `Mathf.RoundToInt(c * 255f)`
/// (round half to even), clamped, as `RRGGBB`.
#[must_use]
pub fn color_html(c: &[f32]) -> String {
    let ch = |i: usize| {
        let f = c.get(i).copied().unwrap_or(0.0) * 255.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = f64::from(f).round_ties_even().clamp(0.0, 255.0) as u8;
        v
    };
    Color32 { r: ch(0), g: ch(1), b: ch(2), a: 255 }.html_rgb()
}

/// `BlueprintUnit.Color` as `ColorUtility.ToHtmlStringRGB` writes it.
///
/// # Errors
///
/// For a missing blueprint or colour.
pub fn blueprint_color_html(w: &World, bp: Guid) -> Result<String, TextError> {
    let b = w
        .content
        .get(bp)
        .ok_or_else(|| TextError::Unsupported(format!("unit blueprint {bp} missing")))?;
    match b.view().node("Color") {
        Some(Node::Floats(f)) => Ok(color_html(f)),
        _ => Err(TextError::Unsupported("BlueprintUnit.Color missing".into())),
    }
}

fn ui_root(w: &World) -> Result<std::rc::Rc<crate::content::Blueprint>, TextError> {
    w.content
        .get(crate::unit::BLUEPRINT_ROOT)
        .ok_or_else(|| TextError::Unsupported("BlueprintRoot missing".into()))
}

fn ui_colors<'a>(
    root: &'a crate::content::Blueprint,
    section: &str,
) -> Result<View<'a>, TextError> {
    root.view()
        .obj("UIRoot")
        .and_then(|u| u.obj(section))
        .ok_or_else(|| TextError::Unsupported(format!("BlueprintRoot.UIRoot.{section} missing")))
}

impl Env for WorldEnv<'_> {
    fn mf_gender(&mut self) -> Result<Gender, TextError> {
        let u = self.w.dialog.acting_unit.or(self.w.main_character);
        Ok(u.map_or(Gender::Male, |u| gender(self.w, u)))
    }

    fn player_is_king(&mut self) -> Result<bool, TextError> {
        let root = ui_root(self.w)?;
        Ok(root
            .view()
            .guid("m_KingFlag")
            .or_else(|| root.view().guid("KingFlag"))
            .is_some_and(|f| crate::narrative::flags::is_unlocked(self.w, f)))
    }

    fn name(&mut self) -> Result<String, TextError> {
        let u = self
            .w
            .dialog
            .acting_unit
            .or(self.w.main_character)
            .ok_or_else(|| TextError::Throws("{name}: no main character".into()))?;
        character_name(self.w, u)
    }

    fn race_name(&mut self) -> Result<String, TextError> {
        let mc = self
            .w
            .main_character
            .ok_or_else(|| TextError::Throws("{race}: no main character".into()))?;
        let race = self
            .w
            .unit(mc)
            .and_then(|u| u.progression.race)
            .ok_or_else(|| TextError::Throws("{race}: no race".into()))?;
        let b = self
            .w
            .content
            .get(race)
            .ok_or_else(|| TextError::Unsupported(format!("race blueprint {race} missing")))?;
        localized_field(self.w, b.view(), "m_DisplayName")
    }

    fn narrator_color(&mut self) -> Result<Color32, TextError> {
        let root = ui_root(self.w)?;
        color32(ui_colors(&root, "DialogColors")?, "Narrator")
    }

    fn tooltip_color(&mut self, ty: TooltipType) -> Result<Color32, TextError> {
        let book = crate::dialog::current_dialog_is_book(self.w);
        let root = ui_root(self.w)?;
        let c = ui_colors(&root, if book { "BookEventColors" } else { "DialogColors" })?;
        color32(
            c,
            match ty {
                TooltipType::Glossary => "GlossaryGlossary",
                TooltipType::Decisions => "GlossaryDecisions",
                TooltipType::Mechanics => "GlossaryMechanics",
            },
        )
    }
}

/// `DialogFormats` (`Kingmaker.Blueprints.Root/DialogFormats.cs`), filled as `string.Format`.
pub mod formats {
    /// `SpeakerFormatWithColorName`: `<b><color=#{1}>{2}</color></b>:<color=#{4}>{0}</color> {3}`.
    #[must_use]
    pub fn speaker_with_color_name(
        p0: &str,
        color: &str,
        name: &str,
        text: &str,
        narrator: &str,
    ) -> String {
        format!("<b><color=#{color}>{name}</color></b>:<color=#{narrator}>{p0}</color> {text}")
    }

    /// `SpeakerFormatWithName`: `<b>{1}</b>:<color=#{3}>{0}</color> {2}`.
    #[must_use]
    pub fn speaker_with_name(p0: &str, name: &str, text: &str, narrator: &str) -> String {
        format!("<b>{name}</b>:<color=#{narrator}>{p0}</color> {text}")
    }

    /// `AnswerFormatWithColorName`: `<b><color=#{0}>{1}</color></b>: {2}`.
    #[must_use]
    pub fn answer_with_color_name(color: &str, name: &str, text: &str) -> String {
        format!("<b><color=#{color}>{name}</color></b>: {text}")
    }
}

/// `ColorUtility.ToHtmlStringRGB(UIRoot.DialogColors.Narrator)`.
///
/// # Errors
///
/// When the root or the colour is missing.
pub fn narrator_html(w: &World) -> Result<String, TextError> {
    Ok(WorldEnv { w }.narrator_color()?.html_rgb_via_color())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_color_html() {
        assert_eq!(color_html(&[0.15, 0.15, 0.15, 1.0]), "262626");
        assert_eq!(color_html(&[1.0, 0.0, 0.5, 1.0]), "FF0080");
        assert_eq!(color_html(&[2.0, -1.0, 0.0]), "FF0000");
    }
}
