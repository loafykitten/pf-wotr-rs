//! Names the log shows (decompiled, not copied: `Kingmaker.Items/ItemEntity.cs` `Name`,
//! `Kingmaker.Blueprints.Items/BlueprintItem.cs` `Name`/`NonIdentifiedName`,
//! `Kingmaker.Blueprints.Items.Weapons/BlueprintItemWeapon.cs` `Name`,
//! `GetEnchantmentPrefixes`/`GetEnchantmentSuffixes`).

use wr_text::TextError;

use crate::content::View;
use crate::world::{EntityIdx, World};

fn bp(w: &World, g: wr_content::Guid) -> Result<std::rc::Rc<crate::content::Blueprint>, TextError> {
    w.content.get(g).ok_or_else(|| TextError::Unsupported(format!("blueprint {g} missing")))
}

fn loc(w: &World, v: View<'_>, field: &str) -> Result<String, TextError> {
    crate::text::localized_field(w, v, field)
}

/// `ItemEntity.Name`: the blueprint's `Name` when identified, else its `NonIdentifiedName`.
///
/// # Errors
///
/// What the sim can't name (the default unidentified names, a weapon without a name of its
/// own or its type's).
pub fn item_name(w: &World, item: EntityIdx) -> Result<String, TextError> {
    let it = w.item(item).ok_or_else(|| TextError::Unsupported("not an item".into()))?;
    let g = it.blueprint.ok_or_else(|| TextError::Unsupported("item without blueprint".into()))?;
    let b = bp(w, g)?;
    let v = b.view();
    if !it.is_identified {
        // `m_NonIdentifiedNameText.IsSet()`: the pack's text for its key isn't empty.
        let n = loc(w, v, "m_NonIdentifiedNameText")?;
        if !n.is_empty() {
            return Ok(n);
        }
        // `DefaultNonIdentifiedName`: a weapon's is its type's `DefaultName`.
        if v.is("BlueprintItemWeapon") {
            let ty = bp(w, v.guid("m_Type").ok_or_else(|| TextError::Unsupported("weapon without type".into()))?)?;
            return loc(w, ty.view(), "m_DefaultNameText");
        }
        return Err(TextError::Unsupported(format!(
            "an unidentified item's default name (UnidentifiedItemNames; item {}{})",
            w.entity(item).unique_id,
            if it.runtime_created { ", created in the run" } else { "" }
        )));
    }
    let base = loc(w, v, "m_DisplayNameText")?;
    if !v.is("BlueprintItemWeapon") || !base.is_empty() {
        return Ok(base);
    }
    // `BlueprintItemWeapon.Name`: the type's default name with the enchantments' prefixes
    // (each followed by a space) and suffixes (each after one).
    let ty = bp(w, v.guid("m_Type").ok_or_else(|| TextError::Unsupported("weapon without type".into()))?)?;
    let default = loc(w, ty.view(), "m_DefaultNameText")?;
    let (mut pre, mut suf) = (String::new(), String::new());
    for e in v.guids("m_Enchantments") {
        let eb = bp(w, e)?;
        let p = loc(w, eb.view(), "m_Prefix")?;
        if !p.is_empty() {
            pre.push_str(&p);
            pre.push(' ');
        }
        let s = loc(w, eb.view(), "m_Suffix")?;
        if !s.is_empty() {
            suf.push(' ');
            suf.push_str(&s);
        }
    }
    let name = format!("{pre}{default}{suf}");
    if !name.is_empty() {
        return Ok(name);
    }
    let type_name = loc(w, ty.view(), "m_TypeNameText")?;
    if !type_name.is_empty() {
        return Ok(type_name);
    }
    Err(TextError::Unsupported("a weapon without a name, default name or type name".into()))
}
