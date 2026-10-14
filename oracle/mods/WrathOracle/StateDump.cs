using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using HarmonyLib;
using Kingmaker;
using Kingmaker.AreaLogic.Etudes;
using Kingmaker.Blueprints;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.EntitySystem.Stats;
using Kingmaker.Items;
using Kingmaker.Items.Slots;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Buffs;
using Kingmaker.UnitLogic.Mechanics;
using UnityEngine;

namespace WrathOracle;

// The state dump, docs/oracle/state-dump-format.md (schema wrath-oracle-state-v1).
internal static class StateDump
{
    public static long Written;

    public static string Run(string outDir, string label, string saveFile)
    {
        var game = Game.Instance;
        var before = RandomState.Text();
        // Dump-scoped counters; the trace's own counters are restored afterwards.
        long t0 = Snap.Truncated, c0 = Snap.Cycles, v0 = Snap.ValueFailures;
        try
        {
            return RunInner(outDir, label, saveFile, game, before, t0, c0, v0);
        }
        finally
        {
            Snap.Truncated = t0;
            Snap.Cycles = c0;
            Snap.ValueFailures = v0;
        }
    }

    private static Kingmaker.UnitLogic.Abilities.AbilityParams _emptyAtLoad;

    public static void NoteAbilityParamsEmptyAtLoad()
    {
        try { _emptyAtLoad = Kingmaker.UnitLogic.Abilities.AbilityParams.Empty.Clone(); }
        catch (Exception e) { Health.Failed("ability params at load", e); }
    }

    private static void WriteParams(Json j, Kingmaker.UnitLogic.Abilities.AbilityParams p)
    {
        j.Obj();
        j.Prop("SpellSource", (int)p.SpellSource);
        j.Prop("CasterLevel", p.CasterLevel);
        j.Prop("SpellLevel", p.SpellLevel);
        j.Prop("DC", p.DC);
        j.Prop("Concentration", p.Concentration);
        j.Prop("Metamagic", (int)p.Metamagic);
        j.Prop("RankBonus", p.RankBonus);
        j.End();
    }

    private static string RunInner(string outDir, string label, string saveFile, Game game, string before, long t0, long c0, long v0)
    {
        var path = Path.Combine(outDir, $"state-{label}.json");
        var failedBefore = Health.FailedCount;
        using (var j = new Json(path, indent: false))
        {
            j.Obj();
            j.Prop("schema", "wrath-oracle-state-v1");
            j.Prop("label", label);
            j.Key("identity"); IdentityShort(j, saveFile);
            j.Prop("frame", Time.frameCount);
            j.Prop("tick", Trace.Active ? Trace.Tick : -1);
            j.Prop("random_state_before", before);
            // AbilityParams.Empty as the last load began, and now (process state; see Runner.Load).
            if (_emptyAtLoad != null) { j.Key("abilityParamsEmpty"); WriteParams(j, _emptyAtLoad); }
            j.Key("abilityParamsEmptyNow"); WriteParams(j, Kingmaker.UnitLogic.Abilities.AbilityParams.Empty);
            j.Key("game"); j.Record("game", x => Game_(x, game));
            j.Arr("units");
            foreach (var u in AllUnits(game)) j.Record("unit " + u.UniqueId, x => Unit(x, u));
            j.EndArr();
            j.Key("narrative"); j.Record("narrative", x => Narrative(x, game));
            j.Prop("random_state_after", RandomState.Text());
            j.Obj("health");
            j.Prop("records_failed", Health.FailedCount - failedBefore);
            j.Prop("value_failures", Snap.ValueFailures - v0);
            j.Prop("truncated", Snap.Truncated - t0);
            j.Prop("cycles", Snap.Cycles - c0);
            j.Prop("random_state_unchanged", before == RandomState.Text());
            j.End();
            j.End();
        }
        Written++;
        return path;
    }

    private static string _assemblySha, _packSha, _saveSha, _saveFileHashed;

    private static void IdentityShort(Json j, string saveFile)
    {
        var contents = Application.dataPath;
        var appRoot = Path.GetDirectoryName(contents);
        _assemblySha = _assemblySha ?? Json.Sha256File(typeof(Game).Assembly.Location);
        _packSha = _packSha ?? Json.Sha256File(Path.Combine(appRoot, "Bundles", "blueprints-pack.bbp"));
        if (_saveFileHashed != saveFile) { _saveSha = saveFile != null ? Json.Sha256File(saveFile) : null; _saveFileHashed = saveFile; }
        j.Obj();
        j.Prop("oracle_version", Main.Mod.Info.Version);
        j.Prop("game_version", SafeStr(() => Kingmaker.GameVersion.GetVersion()));
        j.Prop("game_revision", SafeStr(() => Kingmaker.QA.GameRevision.Value));
        j.Prop("assembly_csharp_sha256", _assemblySha);
        j.Prop("blueprints_pack_sha256", _packSha);
        j.Prop("save_file", saveFile != null ? Path.GetFileName(saveFile) : null);
        j.Prop("save_sha256", _saveSha);
        j.Arr("mods");
        foreach (var m in UnityModManagerNet.UnityModManager.modEntries)
            j.Obj().Prop("id", m.Info.Id).Prop("version", m.Info.Version).Prop("active", m.Active || m == Main.Mod).End();
        j.EndArr();
        j.Obj("difficulty");
        try
        {
            var group = Kingmaker.Settings.SettingsRoot.Difficulty;
            foreach (var f in group.GetType().GetFields(BindingFlags.Instance | BindingFlags.Public))
            {
                var e = f.GetValue(group);
                if (!(e is Kingmaker.Settings.ISettingsEntity)) continue;
                var key = e.GetType().GetProperty("Key")?.GetValue(e) as string ?? f.Name;
                j.Prop(key, e.GetType().GetMethod("GetValue", Type.EmptyTypes)?.Invoke(e, null)?.ToString());
            }
        }
        catch (Exception e) { Health.Failed("difficulty settings", e); }
        j.End();
        j.End();
    }

    private static string SafeStr(Func<string> f)
    {
        try { return f(); } catch (Exception) { return null; }
    }

    public static IEnumerable<UnitEntityData> AllUnits(Game game)
    {
        var seen = new HashSet<UnitEntityData>();
        var list = new List<UnitEntityData>();
        foreach (var e in game.State.Entities.All) if (e is UnitEntityData u && seen.Add(u)) list.Add(u);
        var area = game.State.LoadedAreaState;
        if (area != null) foreach (var e in area.AllEntityData) if (e is UnitEntityData u && seen.Add(u)) list.Add(u);
        foreach (var u in game.Player.AllCrossSceneUnits) if (seen.Add(u)) list.Add(u);
        return list.OrderBy(u => u.UniqueId, StringComparer.Ordinal);
    }

    private static void Game_(Json j, Game game)
    {
        var p = game.Player;
        j.Obj();
        j.Prop("game_time", p.GameTime.Ticks);
        j.Prop("real_time", p.RealTime.Ticks);
        j.Prop("mode", (int)game.CurrentMode);
        j.Prop("mode_name", game.CurrentMode.ToString());
        j.Prop("area", Snap.Guid(game.CurrentlyLoadedArea));
        j.Prop("area_part", Snap.Guid(game.CurrentlyLoadedAreaPart));
        j.Key("main_character"); Snap.UnitRef(j, p.MainCharacter.Value);
        Units(j, "party", p.Party);
        Units(j, "party_and_pets", p.PartyAndPets);
        Units(j, "all_characters", p.AllCharacters);
        j.Prop("money", p.Money);
        j.Arr("shared_inventory");
        foreach (var it in p.Inventory) Item(j, it);
        j.EndArr();
        j.Prop("turn_based", Hooks.InTurnBasedCombat());
        j.End();
    }

    private static void Units(Json j, string key, IEnumerable<UnitEntityData> units)
    {
        j.Arr(key);
        foreach (var u in units) Snap.UnitRef(j, u);
        j.EndArr();
    }

    private static readonly FieldInfo Conditions = AccessTools.Field(typeof(UnitState), "m_Conditions");
    private static readonly FieldInfo Immunities = AccessTools.Field(typeof(UnitState), "m_ConditionsImmunity");

    // Descriptor members written elsewhere in the unit entry (by their JSON names).
    private static readonly HashSet<string> UnitListed = new HashSet<string>
    {
        "Stats", "Resources", "Progression", "State", "Alignment", "m_Inventory", "Inventory", "Body", "m_Spellbooks", "Spellbooks",
        "Facts", "Buffs", "Abilities", "ActivatableAbilities", "UniqueId", "Blueprint", "m_Blueprint",
    };

    // UnitEntityData members written elsewhere (the descriptor, identity, pose, facts).
    private static readonly HashSet<string> EntityListed = new HashSet<string>
    {
        "Descriptor", "UniqueId", "Position", "m_Position", "m_Orientation", "Orientation", "Facts", "m_Facts",
    };

    private static void Unit(Json j, UnitEntityData u)
    {
        var d = u.Descriptor;
        j.Obj();
        j.Prop("id", u.UniqueId);
        j.Prop("blueprint", Snap.Guid(u.Blueprint));
        j.Prop("blueprint_name", u.Blueprint?.name);
        j.Prop("original_blueprint", Snap.Guid(d.OriginalBlueprint));
        j.Prop("class", u.GetType().Name);
        j.Obj("flags");
        j.Prop("in_game", u.IsInGame);
        j.Prop("is_in_state", u.IsInState);
        j.Prop("is_revealed", u.IsRevealed);
        j.Prop("is_main_character", u.IsMainCharacter);
        j.Prop("is_pet", u.IsPet);
        j.Prop("in_party", Game.Instance.Player.Party.Contains(u));
        j.Prop("is_player_faction", u.IsPlayerFaction);
        j.Prop("is_dead", u.State.IsDead);
        j.Prop("is_conscious", u.State.IsConscious);
        j.Prop("is_turned_on", u.IsTurnedOn);
        j.Prop("is_extra", u.IsExtra);
        j.End();
        j.Key("master"); Snap.UnitRef(j, u.Master);
        j.Prop("faction", Snap.Guid(u.Faction));
        j.Prop("position", u.Position);
        j.Prop("orientation", u.Orientation);
        j.Prop("size", (int)u.State.Size);
        j.Prop("original_size", (int)d.OriginalSize);
        j.Obj("hp");
        j.Prop("max", d.MaxHP);
        j.Prop("damage", d.Damage);
        j.Prop("nonlethal", d.DamageNonLethal);
        j.Prop("temporary", d.TemporaryHP);
        j.Prop("left", d.HPLeft);
        j.End();
        CondArray(j, "conditions", Conditions?.GetValue(u.State) as sbyte[]);
        CondArray(j, "condition_immunities", Immunities?.GetValue(u.State) as sbyte[]);
        j.Key("alignment");
        Extended(j, d.Alignment, x => { x.Prop("value", (int)d.Alignment.ValueRaw); x.Prop("value_name", d.Alignment.ValueRaw.ToString()); });
        j.Key("progression");
        Extended(j, d.Progression, x => { x.Prop("character_level", d.Progression.CharacterLevel); x.Prop("mythic_level", d.Progression.MythicLevel); });

        j.Arr("stats");
        foreach (var s in d.Stats.AllStats.Distinct().OrderBy(s => (int)s.Type)) Stat(j, s);
        j.EndArr();

        j.Arr("facts");
        var facts = u.Facts.List;
        for (var i = 0; i < facts.Count; i++) Fact(j, facts[i], i);
        j.EndArr();

        j.Arr("resources");
        foreach (var r in d.Resources.PersistantResources.OrderBy(r => Snap.Guid(r.Blueprint), StringComparer.Ordinal))
        {
            j.Obj();
            j.Prop("blueprint", Snap.Guid(r.Blueprint));
            j.Prop("amount", r.Amount);
            j.Key("persisted"); j.W.WriteRawValue(Snap.ValueText(r, Snap.Mode.Persisted, 0));
            j.End();
        }
        j.EndArr();

        j.Arr("spellbooks");
        foreach (var sb in d.Spellbooks.OrderBy(s => Snap.Guid(s.Blueprint), StringComparer.Ordinal))
        {
            j.Obj();
            j.Prop("blueprint", Snap.Guid(sb.Blueprint));
            j.Prop("caster_level", sb.CasterLevel);
            j.Key("persisted"); j.W.WriteRawValue(Snap.ValueText(sb, Snap.Mode.Persisted, 0));
            j.End();
        }
        j.EndArr();

        j.Key("body"); Body(j, d.Body);
        j.Key("inventory");
        if (d.HasOwnInventory && d.Inventory != null)
        {
            j.W.WriteStartArray();
            foreach (var it in d.Inventory) Item(j, it);
            j.W.WriteEndArray();
        }
        else j.W.WriteNull();
        j.Key("state"); j.W.WriteRawValue(Snap.ValueText(u.State, Snap.Mode.Persisted, 0));
        j.Key("persisted"); PersistedSkipping(j, d, UnitListed);
        j.Key("entity_persisted"); PersistedSkipping(j, u, EntityListed);
        j.End();
    }

    private static void PersistedSkipping(Json j, object o, ICollection<string> skip)
    {
        var sw = new StringWriter(System.Globalization.CultureInfo.InvariantCulture);
        try
        {
            using (var x = new Json(sw)) Snap.Persisted(x, o, skip, 1);
            j.W.WriteRawValue(sw.ToString());
        }
        catch (Exception e)
        {
            Health.Failed("persisted " + o.GetType().Name, e);
            j.W.WriteStartObject(); j.W.WritePropertyName("$failed"); j.W.WriteValue(e.Message); j.W.WriteEndObject();
        }
    }

    // A persisted object with a few extra computed members appended.
    private static void Extended(Json j, object o, Action<Json> extra)
    {
        var text = Snap.ValueText(o, Snap.Mode.Persisted, 0);
        var sw = new StringWriter(System.Globalization.CultureInfo.InvariantCulture);
        using (var x = new Json(sw)) { x.Obj(); extra(x); x.End(); }
        var add = sw.ToString();
        if (text.Length >= 2 && text[0] == '{' && add.Length > 2)
            text = text.Length == 2 ? add : text.Substring(0, text.Length - 1) + "," + add.Substring(1);
        j.W.WriteRawValue(text);
    }

    private static void CondArray(Json j, string key, sbyte[] counts)
    {
        j.Arr(key);
        if (counts != null)
        {
            for (var i = 0; i < counts.Length; i++)
            {
                if (counts[i] == 0) continue;
                j.W.WriteStartArray();
                j.W.WriteValue(i);
                j.W.WriteValue(Enum.GetName(typeof(UnitCondition), i));
                j.W.WriteValue((int)counts[i]);
                j.W.WriteEndArray();
            }
        }
        j.EndArr();
    }

    private static readonly HashSet<string> StatSkip = new HashSet<string>
    {
        "ModifierList", "m_Dependents", "m_Stats", "m_DependentFacts", "m_DependentComponents", "Type", "m_BaseValue",
        "PermanentValue", "ModifiedValue", "ModifiedValueRaw", "BaseValue", "Owner", "Stats", "Modifiers", "OnChanged",
    };

    public static void Stat(Json j, ModifiableValue s)
    {
        j.Obj();
        j.Prop("type", (int)s.Type);
        j.Prop("type_name", s.Type.ToString());
        j.Prop("class", s.GetType().Name);
        j.Prop("base", s.BaseValue);
        j.Prop("permanent", s.PermanentValue);
        j.Prop("modified", s.ModifiedValue);
        j.Prop("raw", s.ModifiedValueRaw);
        j.Key("extra");
        j.W.WriteStartObject();
        foreach (var kv in Snap.FieldPairs(s, 2, StatSkip))
        {
            j.W.WritePropertyName(kv.Key);
            j.W.WriteRawValue(kv.Value);
        }
        j.W.WriteEndObject();
        j.Key("modifiers"); Snap.Modifiers(j, s);
        j.End();
    }

    private static readonly HashSet<string> FactListed = new HashSet<string>
    {
        "UniqueId", "Blueprint", "SourceFact", "SourceItem", "SourceAbility", "IsActive", "AttachTime", "ComponentsConverter", "Rank",
        "Components", "EndTime", "m_EndTime", "NextTickTime", "TickTime", "RoundNumber", "IsSuppressed", "IsFromSpell",
    };

    // For facts other than buffs only the members actually written above are skipped (an item
    // enchantment's EndTime, for instance, stays in "persisted").
    private static readonly HashSet<string> FactListedNonBuff = new HashSet<string>
    {
        "UniqueId", "Blueprint", "SourceFact", "SourceItem", "SourceAbility", "IsActive", "Components",
    };

    private static readonly PropertyInfo ComponentsConverter = AccessTools.Property(typeof(EntityFact), "ComponentsConverter");

    public static void Fact(Json j, EntityFact f, int index)
    {
        j.Obj();
        if (index >= 0) j.Prop("index", index);
        j.Prop("id", f.UniqueId);
        j.Prop("class", f.GetType().Name);
        j.Prop("blueprint", Snap.Guid(f.Blueprint));
        j.Prop("blueprint_name", f.Blueprint?.name);
        j.Prop("rank", f.GetRank());
        j.Prop("active", f.IsActive);
        j.Key("source_fact"); Snap.FactRef(j, f.SourceFact);
        j.Key("source_item"); Snap.Value(j, f.SourceItem, Snap.Mode.Fields, 3);
        j.Prop("source_ability", Snap.Guid(f.SourceAbility));
        j.Key("caster");
        UnitEntityData caster = null;
        var casterNote = (string)null;
        try { caster = f.MaybeContext?.MaybeCaster; } catch (Exception e) { casterNote = e.Message; }
        Snap.UnitRef(j, caster);
        if (casterNote != null) j.Prop("caster_unavailable", casterNote);
        if (f is Buff b)
        {
            j.Obj("buff");
            j.Prop("start_time", b.AttachTime.Ticks);
            j.Key("end_time"); if (b.IsPermanent) j.W.WriteNull(); else j.W.WriteValue(b.EndTime.Ticks);
            j.Prop("next_tick_time", Traverse.Create(b).Property("NextTickTime").GetValue<TimeSpan>().Ticks);
            j.Prop("tick_time", Traverse.Create(b).Property("TickTime").GetValue<TimeSpan>().Ticks);
            j.Prop("round_number", b.RoundNumber);
            j.Prop("is_permanent", b.IsPermanent);
            j.Prop("is_suppressed", b.IsSuppressed);
            j.Prop("is_from_spell", b.IsFromSpell);
            j.End();
        }
        j.Arr("components");
        try
        {
            {
                foreach (var c in f.Components)
                {
                    var rt = c as EntityFactComponent;
                    j.Obj();
                    j.Prop("name", rt?.SourceBlueprintComponent?.name);
                    j.Prop("class", c?.GetType().Name);
                    j.Key("data");
                    var data = (c as IComponentDelegateRuntime)?.Data;
                    j.W.WriteRawValue(data == null ? "null" : Snap.ValueText(data, Snap.Mode.Persisted, 0));
                    j.End();
                }
            }
        }
        catch (Exception e)
        {
            Health.Failed("fact components", e);
        }
        j.EndArr();
        j.Key("persisted"); PersistedSkipping(j, f, f is Buff ? FactListed : FactListedNonBuff);
        j.End();
    }

    private static void Body(Json j, UnitBody body)
    {
        if (body == null) { j.W.WriteNull(); return; }
        j.Obj();
        j.Prop("current_hand_set", body.CurrentHandEquipmentSetIndex);
        j.Arr("hand_sets");
        foreach (var hs in body.HandsEquipmentSets)
        {
            j.Obj();
            j.Key("primary"); Slot(j, hs.PrimaryHand);
            j.Key("secondary"); Slot(j, hs.SecondaryHand);
            j.End();
        }
        j.EndArr();
        j.Obj("slots");
        foreach (var f in typeof(UnitBody).GetFields(BindingFlags.Instance | BindingFlags.Public))
        {
            if (typeof(ItemSlot).IsAssignableFrom(f.FieldType)) { j.Key(f.Name); Slot(j, f.GetValue(body) as ItemSlot); }
            else if (f.FieldType == typeof(UsableSlot[]) && f.GetValue(body) is UsableSlot[] quick)
                for (var i = 0; i < quick.Length; i++) { j.Key("QuickSlot" + i); Slot(j, quick[i]); }
        }
        j.End();
        // Weapons outside the regular slots (empty-hand weapons, extra limbs, a polymorph's
        // hand set) take part in attacks; dump them as full items, not just references.
        j.Key("empty_hand_weapon");
        if (BodyEmptyHand.GetValue(body) is ItemEntity eh) Item(j, eh); else j.W.WriteNull();
        j.Arr("empty_hand_weapons_stack");
        if (BodyEmptyHandStack.GetValue(body) is List<ItemEntityWeapon> stack) foreach (var w in stack) { if (w == null) j.W.WriteNull(); else Item(j, w); }
        j.EndArr();
        j.Arr("additional_limbs");
        if (BodyLimbs.GetValue(body) is List<WeaponSlot> limbs) foreach (var l in limbs) Slot(j, l);
        j.EndArr();
        j.Key("polymorph_hand_set");
        if (BodyPolymorphSet.GetValue(body) is HandsEquipmentSet ps)
        {
            j.Obj();
            j.Key("primary"); Slot(j, ps.PrimaryHand);
            j.Key("secondary"); Slot(j, ps.SecondaryHand);
            j.End();
        }
        else j.W.WriteNull();
        j.Key("polymorph_additional_limbs");
        if (BodyPolymorphLimbs.GetValue(body) is List<WeaponSlot> plimbs)
        {
            j.W.WriteStartArray();
            foreach (var l in plimbs) Slot(j, l);
            j.W.WriteEndArray();
        }
        else j.W.WriteNull();
        j.Key("persisted");
        PersistedSkipping(j, body, new HashSet<string>(typeof(UnitBody).GetFields(BindingFlags.Instance | BindingFlags.Public)
            .Where(f => typeof(ItemSlot).IsAssignableFrom(f.FieldType) || f.FieldType == typeof(UsableSlot[])).Select(f => f.Name)) { "m_HandsEquipmentSets", "m_CurrentHandsEquipmentSetIndex" });
        j.End();
    }

    private static readonly FieldInfo BodyEmptyHand = AccessTools.Field(typeof(UnitBody), "m_EmptyHandWeapon");
    private static readonly FieldInfo BodyEmptyHandStack = AccessTools.Field(typeof(UnitBody), "m_EmptyHandWeaponsStack");
    private static readonly FieldInfo BodyLimbs = AccessTools.Field(typeof(UnitBody), "m_AdditionalLimbs");
    private static readonly FieldInfo BodyPolymorphSet = AccessTools.Field(typeof(UnitBody), "m_PolymorphHandsEquipmentSet");
    private static readonly FieldInfo BodyPolymorphLimbs = AccessTools.Field(typeof(UnitBody), "m_PolymorphAdditionalLimbs");

    private static void Slot(Json j, ItemSlot s)
    {
        if (s == null) { j.W.WriteNull(); return; }
        j.Obj();
        j.Key("item"); if (s.MaybeItem == null) j.W.WriteNull(); else Item(j, s.MaybeItem);
        j.Prop("active", s.Active);
        j.End();
    }

    private static readonly HashSet<string> ItemListed = new HashSet<string> { "UniqueId", "Blueprint", "m_Blueprint", "Count", "IsIdentified", "Charges", "m_Enchantments", "Enchantments" };

    private static void Item(Json j, ItemEntity it)
    {
        j.Obj();
        j.Prop("id", it.UniqueId);
        j.Prop("blueprint", Snap.Guid(it.Blueprint));
        j.Prop("blueprint_name", it.Blueprint?.name);
        j.Prop("class", it.GetType().Name);
        j.Prop("count", it.Count);
        j.Prop("is_identified", it.IsIdentified);
        j.Prop("charges", it.Charges);
        j.Arr("enchantments");
        foreach (var e in it.Enchantments) Fact(j, e, -1);
        j.EndArr();
        j.Key("persisted"); PersistedSkipping(j, it, ItemListed);
        j.End();
    }

    private static readonly FieldInfo EtudesData = AccessTools.Field(typeof(EtudesSystem), "m_EtudesData");

    private static void Narrative(Json j, Game game)
    {
        var p = game.Player;
        j.Obj();
        j.Arr("etudes");
        if (EtudesData?.GetValue(p.EtudesSystem) is IDictionary data)
        {
            var rows = new List<(string guid, int state, string name)>();
            foreach (DictionaryEntry e in data) rows.Add((Snap.Guid(e.Key as SimpleBlueprint), Convert.ToInt32(e.Value), e.Value.ToString()));
            foreach (var r in rows.OrderBy(r => r.guid, StringComparer.Ordinal))
                j.Obj().Prop("blueprint", r.guid).Prop("state", r.state).Prop("state_name", r.name).End();
        }
        j.EndArr();
        j.Arr("etude_facts");
        foreach (var f in p.EtudesSystem.Facts.List.OrderBy(f => Snap.Guid(f.Blueprint), StringComparer.Ordinal)) Fact(j, f, -1);
        j.EndArr();
        j.Arr("flags");
        foreach (var kv in p.UnlockableFlags.UnlockedFlags.OrderBy(k => Snap.Guid(k.Key), StringComparer.Ordinal))
            j.Obj().Prop("blueprint", Snap.Guid(kv.Key)).Prop("value", kv.Value).End();
        j.EndArr();
        j.Arr("quests");
        foreach (var q in p.QuestBook.Quests.OrderBy(q => Snap.Guid(q.Blueprint), StringComparer.Ordinal))
        {
            j.Obj();
            j.Prop("blueprint", Snap.Guid(q.Blueprint));
            j.Prop("state", (int)q.State);
            j.Arr("objectives");
            foreach (var o in q.Objectives)
            {
                j.Obj();
                j.Prop("blueprint", Snap.Guid(o.Blueprint));
                j.Prop("state", (int)o.State);
                j.Key("persisted"); j.W.WriteRawValue(Snap.ValueText(o, Snap.Mode.Persisted, 0));
                j.End();
            }
            j.EndArr();
            j.Key("persisted"); PersistedSkipping(j, q, new HashSet<string> { "UniqueId", "Blueprint", "m_Objectives", "ComponentsConverter" });
            j.End();
        }
        j.EndArr();
        j.Obj("dialog");
        var d = p.Dialog;
        GuidSet(j, "SelectedAnswers", d.SelectedAnswers);
        GuidSet(j, "ShownAnswerLists", d.ShownAnswerLists);
        GuidSet(j, "ShownCues", d.ShownCues);
        GuidSet(j, "ShownDialogs", d.ShownDialogs);
        j.Arr("AnswerChecks");
        foreach (var kv in d.AnswerChecks.OrderBy(k => Snap.Guid(k.Key), StringComparer.Ordinal))
        {
            j.W.WriteStartArray();
            j.W.WriteValue(Snap.Guid(kv.Key));
            j.W.WriteRawValue(Snap.ValueText(kv.Value, Snap.Mode.Persisted, 0));
            j.W.WriteEndArray();
        }
        j.EndArr();
        j.Key("Scheduled"); j.W.WriteRawValue(Snap.ValueText(d.Scheduled, Snap.Mode.Persisted, 0));
        j.End();
        var mc = p.MainCharacter.Value;
        j.Key("alignment");
        if (mc != null) Extended(j, mc.Descriptor.Alignment, x => { x.Prop("value", (int)mc.Descriptor.Alignment.ValueRaw); x.Prop("value_name", mc.Descriptor.Alignment.ValueRaw.ToString()); });
        else j.W.WriteNull();
        j.Obj("player");
        j.Prop("chapter", p.Chapter);
        j.Prop("game_id", p.GameId);
        j.Prop("money", p.Money);
        j.End();
        j.End();
    }

    private static void GuidSet(Json j, string key, IEnumerable bps)
    {
        j.Arr(key);
        foreach (var g in bps.Cast<object>().Select(o => Snap.Guid(o as SimpleBlueprint)).OrderBy(x => x, StringComparer.Ordinal)) j.W.WriteValue(g);
        j.EndArr();
    }
}
