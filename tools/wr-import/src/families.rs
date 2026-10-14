//! Classifying scene components (MonoBehaviour script classes and Unity built-ins) into
//! families, with a gameplay-relevance tier used to rank what a port must understand.

use serde::Serialize;

/// How much a component family matters to the fidelity contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relevance {
    /// Feeds rules, story state or entity identity (T0/T1).
    Gameplay,
    /// Affects what the player can do or see in ways rules depend on indirectly
    /// (navigation, perception, fog of war, interaction highlighting).
    Supporting,
    /// Presentation only (T2): lighting, particles, sound, post-processing.
    Presentation,
    /// Editor or build helpers with no runtime effect.
    Inert,
    /// Not classified yet.
    Unknown,
}

/// A component family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Family {
    pub name: &'static str,
    pub relevance: Relevance,
}

const fn fam(name: &'static str, relevance: Relevance) -> Family {
    Family { name, relevance }
}

use Relevance::{Gameplay, Inert, Presentation, Supporting, Unknown};

/// Exact class names (namespace-qualified) first, then namespace prefixes.
const EXACT: &[(&str, Family)] = &[
    ("Kingmaker.View.Spawners.UnitSpawner", fam("spawner", Gameplay)),
    ("Kingmaker.View.Spawners.UnitGroupView", fam("spawner", Gameplay)),
    ("Kingmaker.View.Spawners.CRCalculator", fam("spawner_setting", Gameplay)),
    ("Kingmaker.View.MapObjects.MapObjectView", fam("map_object", Gameplay)),
    ("Kingmaker.View.MapObjects.SriptZones.ScriptZone", fam("script_zone", Gameplay)),
    ("Kingmaker.View.MapObjects.SriptZones.ScriptZoneBox", fam("zone_shape", Gameplay)),
    ("Kingmaker.View.MapObjects.SriptZones.ScriptZoneCylinder", fam("zone_shape", Gameplay)),
    ("Kingmaker.View.MapObjects.SriptZones.ScriptZonePolygon", fam("zone_shape", Gameplay)),
    ("Kingmaker.View.MapObjects.SriptZones.ScriptZoneCircleOnGrid", fam("zone_shape", Gameplay)),
    ("Kingmaker.View.MapObjects.SriptZones.ScriptZoneCustomOnGrid", fam("zone_shape", Gameplay)),
    ("Kingmaker.View.LocatorView", fam("locator", Gameplay)),
    ("Kingmaker.View.AreaEnterPoint", fam("area_enter_point", Gameplay)),
    ("Kingmaker.View.MapObjects.AreaTransition", fam("area_transition", Gameplay)),
    ("Kingmaker.Blueprints.Area.FactHolder", fam("fact_holder", Gameplay)),
    ("Kingmaker.View.PerceptionCheckComponent", fam("perception", Gameplay)),
    ("Kingmaker.View.Roaming.SpawnerRoamingSettings", fam("spawner_setting", Gameplay)),
    ("Kingmaker.UnitLogic.Interaction.SpawnerInteractionBark", fam("spawner_setting", Gameplay)),
    ("Kingmaker.UnitLogic.Interaction.SpawnerInteractionDialog", fam("spawner_setting", Gameplay)),
    ("Kingmaker.View.MapObjects.KingdomLootComponent", fam("interaction", Gameplay)),
    ("Kingmaker.View.MapObjects.MapObjectFxComponent", fam("map_object_fx", Presentation)),
    ("Kingmaker.NavMeshManager", fam("navigation", Supporting)),
    ("Pathfinding.ValidNavmeshArea", fam("navigation", Supporting)),
    ("Pathfinding.NavmeshCut", fam("navigation", Supporting)),
    ("AstarPath", fam("navigation", Supporting)),
    (
        "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.FogOfWarBlocker",
        fam("fog_of_war", Supporting),
    ),
    (
        "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.FogOfWarArea",
        fam("fog_of_war", Supporting),
    ),
    (
        "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.Highlighting.Highlighter",
        fam("highlighting", Supporting),
    ),
    ("Kingmaker.Visual.UnitMultiHighlight", fam("highlighting", Supporting)),
    ("Kingmaker.Localization.SharedStringAsset", fam("localization", Supporting)),
    ("Kingmaker.Utility.ApplyPrefabAtRuntime", fam("marker", Inert)),
    ("Kingmaker.Utility.PlatformDependent", fam("platform_switch", Supporting)),
    ("Kingmaker.Utility.SwitchPrefabs", fam("platform_switch", Supporting)),
    ("Kingmaker.Utility.DistanceController", fam("lod", Presentation)),
    ("Kingmaker.View.Equipment.EquipmentOffsets", fam("equipment_visual", Presentation)),
    ("Kingmaker.View.Equipment.WeaponEquipLinks", fam("equipment_visual", Presentation)),
    ("Kingmaker.Visual.CharacterSystem.AddColliders", fam("character_visual", Presentation)),
    ("UnityEngine.ProBuilder.ProBuilderMesh", fam("editor_mesh", Inert)),
    ("Owlcat.Runtime.Visual.SceneHelpers.TransformFreezer", fam("editor_helper", Inert)),
    ("Owlcat.Runtime.Visual.SceneHelpers.StaticPrefab", fam("editor_helper", Inert)),
    ("Kingmaker.Visual.LightProbesGenerator", fam("editor_helper", Inert)),
];

const PREFIX: &[(&str, Family)] = &[
    ("Kingmaker.View.Spawners.Spawner", fam("spawner_setting", Gameplay)),
    ("Kingmaker.View.Spawners.", fam("spawner_setting", Gameplay)),
    ("Kingmaker.View.MapObjects.Interaction", fam("interaction", Gameplay)),
    ("Kingmaker.View.MapObjects.Traps.", fam("trap", Gameplay)),
    ("Kingmaker.View.MapObjects.", fam("map_object_other", Gameplay)),
    ("Kingmaker.AreaLogic.Cutscenes.", fam("cutscene", Gameplay)),
    ("Kingmaker.View.Cutscenes", fam("cutscene", Gameplay)),
    ("Kingmaker.AreaLogic.", fam("area_logic", Gameplay)),
    ("Kingmaker.UnitLogic.", fam("unit_logic", Gameplay)),
    ("Kingmaker.View.", fam("view_other", Unknown)),
    ("Kingmaker.Sound.", fam("audio", Presentation)),
    ("Kingmaker.Visual.Sound.", fam("audio", Presentation)),
    ("Kingmaker.Visual.Particles.", fam("particles", Presentation)),
    ("Kingmaker.Visual.DayNightCycle.", fam("lighting", Presentation)),
    ("Kingmaker.Visual.Critters.", fam("critters", Presentation)),
    ("Kingmaker.Visual.FoliageInteraction.", fam("foliage", Presentation)),
    ("Kingmaker.Visual.CharactersRigidbody.", fam("character_visual", Presentation)),
    ("Kingmaker.Visual.", fam("visual_other", Presentation)),
    ("Owlcat.Runtime.Visual.RenderPipeline.PostProcess.", fam("post_process", Presentation)),
    (
        "Owlcat.Runtime.Visual.RenderPipeline.IndirectRendering.",
        fam("indirect_rendering", Presentation),
    ),
    ("Owlcat.Runtime.Visual.RenderPipeline.Terrain.", fam("terrain", Presentation)),
    ("Owlcat.Runtime.Visual.RenderPipeline.", fam("render_pipeline", Presentation)),
    ("Owlcat.Runtime.Core.Physics.", fam("cloth_physics", Presentation)),
    ("Owlcat.Runtime.Visual.", fam("visual_other", Presentation)),
    ("UnityEngine.Rendering.", fam("post_process", Presentation)),
    ("Pathfinding.", fam("navigation", Supporting)),
];

/// Classifies a MonoBehaviour by its script's full name.
#[must_use]
pub fn classify_script(full_name: &str) -> Family {
    if let Some((_, f)) = EXACT.iter().find(|(n, _)| *n == full_name) {
        return *f;
    }
    if let Some((_, f)) = PREFIX.iter().find(|(p, _)| full_name.starts_with(p)) {
        return *f;
    }
    match full_name {
        "SoundAnimation" | "AnimatedLight" | "AudioFade" => fam("visual_other", Presentation),
        _ => fam("unclassified", Unknown),
    }
}

/// Classifies a Unity built-in component by class name.
#[must_use]
pub fn classify_builtin(class: &str) -> Family {
    match class {
        "Transform" | "RectTransform" => fam("transform", Gameplay),
        "BoxCollider"
        | "CapsuleCollider"
        | "SphereCollider"
        | "MeshCollider"
        | "TerrainCollider"
        | "CharacterController"
        | "Rigidbody" => fam("physics", Supporting),
        "MeshRenderer" | "SkinnedMeshRenderer" | "MeshFilter" | "LODGroup" | "Terrain" => {
            fam("rendering", Presentation)
        }
        "Light" | "ReflectionProbe" | "LightProbeGroup" => fam("lighting", Presentation),
        "ParticleSystem" | "ParticleSystemRenderer" | "TrailRenderer" | "LineRenderer" => {
            fam("particles", Presentation)
        }
        "Animator" | "Animation" => fam("animation", Presentation),
        "AudioSource" => fam("audio", Presentation),
        "Mesh" | "Material" | "Cubemap" | "Texture2D" | "Texture2DArray" | "Shader" => {
            fam("embedded_asset", Presentation)
        }
        "RenderSettings" | "LightmapSettings" | "LightingSettings" => {
            fam("scene_settings", Presentation)
        }
        // Unity's own NavMesh is unused; the game navigates with the A* graph cache.
        "NavMeshSettings" => fam("unity_navmesh", Inert),
        _ => fam("builtin_other", Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_beats_prefix() {
        assert_eq!(classify_script("Kingmaker.View.Spawners.UnitSpawner").name, "spawner");
        assert_eq!(
            classify_script("Kingmaker.View.Spawners.SpawnerMakePassive").name,
            "spawner_setting"
        );
        assert_eq!(
            classify_script("Kingmaker.View.MapObjects.InteractionLoot").relevance,
            Relevance::Gameplay
        );
        assert_eq!(classify_script("Kingmaker.Visual.Particles.PooledFx").name, "particles");
        assert_eq!(classify_script("Some.Unknown").relevance, Relevance::Unknown);
    }
}
