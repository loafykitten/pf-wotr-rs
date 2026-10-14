using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using Kingmaker;
using Kingmaker.AreaLogic.Cutscenes;
using Kingmaker.Blueprints;
using Kingmaker.Blueprints.Area;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.View;
using Kingmaker.View.MapObjects;
using Kingmaker.View.MapObjects.SriptZones;
using Kingmaker.View.Spawners;
using Pathfinding;
using Path = System.IO.Path;
using UnityEngine;
using UnityEngine.SceneManagement;

namespace WrathOracle;

// Scene and state dump of the loaded area (main thread, between frames; not timed).
//   scene-dump.json: area, loaded scenes and bundles, every EntityViewBase in the loaded
//     scenes (with all components' serialized fields), every EntityDataBase in the game
//     state, and typed lists: spawners, script zones (world-space shapes and containment
//     probes evaluated by the game's own IScriptZoneShape.Contains), locators, area
//     transitions, unit groups, cutscene players, map objects, navmesh cuts.
//   hierarchy/<scene>.json (plan.hierarchy): every GameObject with path, sibling path,
//     activity, layer, world transform and component types.
internal static class SceneDump
{
    public static void Run(string outDir, bool hierarchy, string inventoryPath)
    {
        var game = Game.Instance;
        var scenes = LoadedScenes();
        using (var j = new Json(Path.Combine(outDir, "scene-dump.json")))
        {
            j.Obj();
            j.Prop("schema", "wrath-oracle-scene-dump-v1");
            j.Prop("frame", Time.frameCount);
            j.Key("random_state_before");
            RandomState.Write(j);
            var probes = ZoneProbes.Load(inventoryPath);
            j.Prop("zone_probe_source", probes.Source);
            WriteArea(j, game);
            WriteScenes(j, scenes);

            var views = new List<EntityViewBase>();
            foreach (var s in scenes)
                foreach (var root in s.GetRootGameObjects())
                    views.AddRange(root.GetComponentsInChildren<EntityViewBase>(true));

            j.Arr("views");
            foreach (var v in views) j.Record("view", x => WriteView(x, v));
            j.EndArr();

            j.Arr("entities");
            foreach (var e in AllEntities(game)) j.Record("entity", x => WriteEntity(x, e));
            j.EndArr();

            j.Arr("spawners");
            foreach (var s in views.OfType<UnitSpawnerBase>()) j.Record("spawner", x => WriteSpawner(x, s));
            j.EndArr();

            j.Arr("unit_groups");
            foreach (var g in views.OfType<UnitGroupView>())
                j.Record("unit_group", x =>
                {
                    x.Obj();
                    Common(x, g);
                    x.Arr("spawners");
                    foreach (Transform child in g.transform)
                    {
                        var sp = child.GetComponent<UnitSpawnerBase>();
                        if (sp != null) x.W.WriteValue(sp.UniqueId);
                    }
                    x.EndArr();
                    x.End();
                });
            j.EndArr();

            j.Arr("script_zones");
            foreach (var z in views.OfType<ScriptZone>()) j.Record("script_zone", x => WriteZone(x, z, probes));
            j.EndArr();

            j.Arr("locators");
            foreach (var l in views.OfType<LocatorView>()) j.Record("locator", x => { x.Obj(); Common(x, l); x.End(); });
            j.EndArr();

            j.Arr("area_transitions");
            foreach (var v in views)
            {
                var at = v.GetComponent<AreaTransition>();
                if (at == null) continue;
                j.Record("area_transition", x =>
                {
                    x.Obj();
                    Common(x, v);
                    x.Key("component");
                    FieldDumper.WriteFields(x, at);
                    x.End();
                });
            }
            j.EndArr();

            j.Arr("cutscene_players");
            foreach (var c in game.State.Cutscenes.All)
                j.Record("cutscene_player", x =>
                {
                    x.Obj();
                    x.Prop("unique_id", c.UniqueId);
                    x.Prop("cutscene", c.Cutscene != null ? c.Cutscene.AssetGuid.ToString() : null);
                    x.Prop("cutscene_name", c.Cutscene != null ? c.Cutscene.name : null);
                    x.Prop("holding_state", c.HoldingState?.SceneName);
                    x.Prop("view_path", c.View != null ? Paths.Of(c.View.transform) : null);
                    x.End();
                });
            j.EndArr();

            j.Arr("map_objects");
            foreach (var m in views.OfType<MapObjectView>().Where(m => !(m is ScriptZone)))
                j.Record("map_object", x =>
                {
                    x.Obj();
                    Common(x, m);
                    var parts = m.Data?.Parts?.GetAll<Kingmaker.EntitySystem.EntityPart>();
                    x.Arr("data_parts");
                    if (parts != null) foreach (var p in parts) x.W.WriteValue(p.GetType().FullName);
                    x.EndArr();
                    x.End();
                });
            j.EndArr();

            j.Arr("area_enter_points");
            foreach (var s in scenes)
                foreach (var root in s.GetRootGameObjects())
                    foreach (var ep in root.GetComponentsInChildren<AreaEnterPoint>(true))
                        j.Record("area_enter_point", x =>
                        {
                            x.Obj();
                            x.Prop("scene", s.name);
                            x.Prop("path", Paths.Of(ep.transform));
                            x.Key("sibling_path");
                            Paths.WriteSiblingPath(x, ep.transform);
                            x.Prop("active_in_hierarchy", ep.gameObject.activeInHierarchy);
                            var bp = ep.Blueprint;
                            x.Prop("blueprint", bp != null ? bp.AssetGuid.ToString() : null);
                            x.Prop("blueprint_name", bp != null ? bp.name : null);
                            x.Prop("position", ep.transform.position);
                            x.Prop("rotation", ep.transform.rotation);
                            x.End();
                        });
            j.EndArr();

            j.Arr("navmesh_cuts");
            foreach (var s in scenes)
                foreach (var root in s.GetRootGameObjects())
                    foreach (var cut in root.GetComponentsInChildren<NavmeshCut>(true))
                        j.Record("navmesh_cut", x =>
                        {
                            x.Obj();
                            x.Prop("scene", s.name);
                            x.Prop("path", Paths.Of(cut.transform));
                            x.Prop("active_in_hierarchy", cut.gameObject.activeInHierarchy);
                            x.Prop("enabled", cut.enabled);
                            x.Transform("transform", cut.transform);
                            x.Key("fields");
                            FieldDumper.WriteFields(x, cut);
                            x.End();
                        });
            j.EndArr();

            j.Key("random_state_after");
            RandomState.Write(j);
            Health.Write(j);
            j.End();
        }

        if (!hierarchy) return;
        foreach (var s in scenes)
        {
            var safe = string.Concat(s.name.Select(ch => char.IsLetterOrDigit(ch) || ch == '_' || ch == '-' ? ch : '_'));
            using (var j = new Json(Path.Combine(outDir, "hierarchy", safe + ".json")))
            {
                j.Obj();
                j.Prop("schema", "wrath-oracle-hierarchy-v1");
                j.Prop("scene", s.name);
                j.Prop("scene_path", s.path);
                j.Arr("objects");
                foreach (var root in s.GetRootGameObjects())
                    foreach (var t in root.GetComponentsInChildren<Transform>(true))
                        j.Record("hierarchy object", x => WriteObject(x, t));
                j.EndArr();
                j.End();
            }
        }
    }

    internal static List<Scene> LoadedScenes()
    {
        var list = new List<Scene>();
        for (var i = 0; i < SceneManager.sceneCount; i++)
        {
            var s = SceneManager.GetSceneAt(i);
            if (s.isLoaded) list.Add(s);
        }
        return list;
    }

    private static void WriteArea(Json j, Game game)
    {
        var area = game.CurrentlyLoadedArea;
        var part = game.CurrentlyLoadedAreaPart;
        j.Obj("area");
        j.Prop("guid", area != null ? area.AssetGuid.ToString() : null);
        j.Prop("name", area != null ? area.name : null);
        j.Prop("part_guid", part != null ? part.AssetGuid.ToString() : null);
        j.Prop("part_name", part != null ? part.name : null);
        j.Arr("part_scenes");
        if (part != null)
            foreach (var sr in part.GetStaticAndActiveDynamicScenes())
                j.W.WriteValue(sr?.SceneName);
        j.EndArr();
        if (part != null)
        {
            j.Prop("light_scene", part.GetLightScene(game.TimeOfDay)?.SceneName);
            j.Prop("audio_scene", part.GetAudioScene(game.TimeOfDay)?.SceneName);
        }
        j.Prop("time_of_day", game.TimeOfDay.ToString());
        j.Prop("game_time", game.TimeController.GameTime.ToString());
        j.Prop("real_time", game.TimeController.RealTime.ToString());
        j.Prop("game_mode", game.CurrentMode.ToString());
        j.Prop("paused", game.IsPaused);
        j.Prop("loaded_area_state_scenes", string.Join(",", game.State.LoadedAreaState?.GetAllSceneStates().Select(s => s.SceneName) ?? new string[0]));
        j.End();
    }

    private static void WriteScenes(Json j, List<Scene> scenes)
    {
        j.Arr("scenes");
        foreach (var s in scenes)
        {
            j.Obj();
            j.Prop("name", s.name);
            j.Prop("path", s.path);
            j.Prop("build_index", s.buildIndex);
            j.Prop("root_count", s.rootCount);
            var counts = new SortedDictionary<string, int>(StringComparer.Ordinal);
            var objects = 0;
            j.Arr("roots");
            foreach (var root in s.GetRootGameObjects())
            {
                j.W.WriteValue(root.name);
                foreach (var t in root.GetComponentsInChildren<Transform>(true))
                {
                    objects++;
                    foreach (var c in t.GetComponents<Component>())
                    {
                        var n = c == null ? "(missing script)" : c.GetType().FullName;
                        counts[n] = counts.TryGetValue(n, out var k) ? k + 1 : 1;
                    }
                }
            }
            j.EndArr();
            j.Prop("object_count", objects);
            j.Obj("component_counts");
            foreach (var kv in counts) j.Prop(kv.Key, kv.Value);
            j.End();
            j.End();
        }
        j.EndArr();
        j.Arr("loaded_bundles");
        foreach (var b in AssetBundle.GetAllLoadedAssetBundles().Select(b => b.name).OrderBy(n => n, StringComparer.Ordinal))
            j.W.WriteValue(b);
        j.EndArr();
    }

    // Identity, location and transform shared by all view records.
    private static void Common(Json j, EntityViewBase v)
    {
        j.Prop("class", v.GetType().FullName);
        j.Prop("unique_id", v.UniqueId);
        j.Prop("scene", v.gameObject.scene.name);
        j.Prop("name", v.name);
        j.Prop("path", Paths.Of(v.transform));
        j.Key("sibling_path");
        Paths.WriteSiblingPath(j, v.transform);
        j.Prop("active_in_hierarchy", v.gameObject.activeInHierarchy);
        j.Prop("position", v.transform.position);
        j.Prop("rotation", v.transform.rotation);
    }

    private static void WriteView(Json j, EntityViewBase v)
    {
        j.Obj();
        Common(j, v);
        j.Prop("active_self", v.gameObject.activeSelf);
        j.Prop("layer", v.gameObject.layer);
        j.Prop("tag", v.gameObject.tag);
        j.Prop("is_in_game_by_settings", v.IsInGameBySettings);
        j.Prop("data_unique_id", v.Data?.UniqueId);
        j.Prop("data_type", v.Data?.GetType().FullName);
        j.Prop("data_is_in_game", v.Data != null ? v.Data.IsInGame : (bool?)null);
        j.Prop("data_is_in_state", v.Data != null ? v.Data.IsInState : (bool?)null);
        j.Prop("data_is_revealed", v.Data != null ? v.Data.IsRevealed : (bool?)null);
        j.Transform("transform", v.transform);
        // Units are spawned at runtime from prefabs; their prefab components are asset
        // data rather than scene data, so list them by type only.
        var full = !(v is UnitEntityView);
        j.Arr("components");
        foreach (var c in v.GetComponents<Component>())
        {
            j.Obj();
            if (c == null)
            {
                j.Prop("type", "(missing script)");
                j.End();
                continue;
            }
            j.Prop("type", c.GetType().FullName);
            if (c is Behaviour b) j.Prop("enabled", b.enabled);
            if (full && c is MonoBehaviour)
            {
                j.Key("fields");
                FieldDumper.WriteFields(j, c);
            }
            else if (c is Collider col)
            {
                WriteCollider(j, col);
            }
            j.End();
        }
        j.EndArr();
        j.End();
    }

    private static void WriteCollider(Json j, Collider col)
    {
        j.Prop("collider_enabled", col.enabled);
        j.Prop("is_trigger", col.isTrigger);
        j.Prop("world_bounds", col.bounds);
        switch (col)
        {
            case BoxCollider bc: j.Prop("center", bc.center).Prop("size", bc.size); break;
            case SphereCollider sc: j.Prop("center", sc.center).Prop("radius", sc.radius); break;
            case CapsuleCollider cc: j.Prop("center", cc.center).Prop("radius", cc.radius).Prop("height", cc.height).Prop("direction", cc.direction); break;
            case MeshCollider mc: j.Prop("mesh", mc.sharedMesh != null ? mc.sharedMesh.name : null).Prop("convex", mc.convex); break;
        }
    }

    private static IEnumerable<EntityDataBase> AllEntities(Game game)
    {
        var seen = new HashSet<EntityDataBase>();
        foreach (var e in game.State.Entities.All)
            if (seen.Add(e)) yield return e;
        var area = game.State.LoadedAreaState;
        if (area != null)
            foreach (var e in area.AllEntityData)
                if (seen.Add(e)) yield return e;
    }

    private static readonly Dictionary<Type, (PropertyInfo prop, FieldInfo field)> BlueprintMember = new Dictionary<Type, (PropertyInfo, FieldInfo)>();

    private static SimpleBlueprint BlueprintOf(object o)
    {
        var t = o.GetType();
        if (!BlueprintMember.TryGetValue(t, out var m))
        {
            var p = t.GetProperty("Blueprint", BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
            if (p != null && !typeof(SimpleBlueprint).IsAssignableFrom(p.PropertyType)) p = null;
            FieldInfo f = null;
            for (var c = t; c != null && f == null; c = c.BaseType)
                f = c.GetField("m_Blueprint", BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly);
            m = (p, f);
            BlueprintMember[t] = m;
        }
        try
        {
            if (m.prop != null) return m.prop.GetValue(o) as SimpleBlueprint;
            var v = m.field?.GetValue(o);
            return v as SimpleBlueprint ?? (v as BlueprintReferenceBase)?.GetBlueprint();
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static void WriteEntity(Json j, EntityDataBase e)
    {
        j.Obj();
        j.Prop("unique_id", e.UniqueId);
        j.Prop("type", e.GetType().FullName);
        var bp = e is CutscenePlayerData cp ? cp.Cutscene : BlueprintOf(e);
        j.Prop("blueprint", bp != null ? bp.AssetGuid.ToString() : null);
        j.Prop("blueprint_name", bp != null ? bp.name : null);
        j.Prop("holding_state", e.HoldingState?.SceneName);
        j.Prop("is_in_game", e.IsInGame);
        j.Prop("is_turned_on", e.IsTurnedOn);
        j.Prop("is_in_state", e.IsInState);
        j.Prop("is_revealed", e.IsRevealed);
        j.Prop("has_view", e.View != null);
        j.Prop("destroyed", e.Destroyed);
        if (e.View != null)
        {
            j.Prop("view_class", e.View.GetType().FullName);
            j.Prop("view_scene", e.View.gameObject.scene.name);
            j.Prop("view_path", Paths.Of(e.View.transform));
            j.Prop("view_position", e.View.transform.position);
            j.Prop("view_rotation", e.View.transform.rotation);
        }
        switch (e)
        {
            case UnitEntityData u:
                j.Prop("position", u.Position);
                j.Prop("orientation", u.Orientation);
                j.Prop("character_name", u.CharacterName);
                j.Prop("is_player_faction", u.IsPlayerFaction);
                j.Prop("faction", u.Faction != null ? u.Faction.AssetGuid.ToString() : null);
                j.Prop("corpulence", u.Corpulence);
                j.Prop("is_dead", u.State.IsDead);
                j.Prop("in_party", Game.Instance.Player.PartyCharacters.Any(r => r.Value == u));
                break;
            case UnitPlaceholderEntityData ph:
                j.Prop("placeholder_unit_id", Field<string>(ph, "m_Id"));
                j.Prop("position", Field<Vector3>(ph, "m_Position"));
                break;
            case UnitSpawnerBase.MyData sd:
                j.Prop("has_spawned", sd.HasSpawned);
                j.Prop("has_died", sd.HasDied);
                j.Prop("spawned_unit", sd.SpawnedUnit.Value?.UniqueId);
                break;
        }
        j.End();
    }

    private static T Field<T>(object o, string name)
    {
        for (var c = o.GetType(); c != null; c = c.BaseType)
        {
            var f = c.GetField(name, BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly);
            if (f != null) return (T)f.GetValue(o);
        }
        return default;
    }

    private static void WriteSpawner(Json j, UnitSpawnerBase s)
    {
        j.Obj();
        Common(j, s);
        var bp = s.Blueprint;
        j.Prop("blueprint", bp != null ? bp.AssetGuid.ToString() : null);
        j.Prop("blueprint_name", bp != null ? bp.name : null);
        j.Prop("spawn_on_scene_init", s.SpawnOnSceneInit);
        j.Prop("has_spawned", s.Data != null ? s.Data.HasSpawned : (bool?)null);
        j.Prop("has_died", s.Data != null ? s.Data.HasDied : (bool?)null);
        var unit = s.SpawnedUnit;
        j.Prop("spawned_unit", unit?.UniqueId);
        if (unit != null)
        {
            j.Prop("spawned_unit_blueprint", unit.Blueprint != null ? unit.Blueprint.AssetGuid.ToString() : null);
            j.Prop("spawned_unit_position", unit.Position);
            j.Prop("spawned_unit_orientation", unit.Orientation);
        }
        var group = s.transform.parent != null ? s.transform.parent.GetComponent<UnitGroupView>() : null;
        j.Prop("group", group != null ? group.UniqueId : null);
        j.Arr("settings_components");
        foreach (var c in s.GetComponents<MonoBehaviour>())
        {
            if (c == null || c == s) continue;
            j.Obj().Prop("type", c.GetType().FullName);
            j.Key("fields");
            FieldDumper.WriteFields(j, c);
            j.End();
        }
        j.EndArr();
        j.End();
    }

    private static void WriteZone(Json j, ScriptZone z, ZoneProbes probes)
    {
        j.Obj();
        Common(j, z);
        var bp = z.Blueprint;
        j.Prop("blueprint", bp != null ? bp.AssetGuid.ToString() : null);
        j.Prop("blueprint_name", bp != null ? bp.name : null);
        j.Prop("is_active", z.Data is ScriptZone.ScriptZoneData zd ? zd.IsActive : (bool?)null);
        j.Prop("once_only", z.OnceOnly);
        j.Prop("players_only", Field<bool>(z, "m_PlayersOnly"));
        j.Prop("collected_shape_count", z.Shapes.Count);
        j.Arr("shapes");
        var collected = new HashSet<IScriptZoneShape>(z.Shapes);
        var shapeIndex = -1;
        foreach (var shape in z.GetComponentsInChildren<IScriptZoneShape>(true))
        {
            shapeIndex++;
            var c = (Component)shape;
            j.Obj();
            j.Prop("type", c.GetType().FullName);
            j.Prop("path", Paths.Of(c.transform));
            j.Prop("active_in_hierarchy", c.gameObject.activeInHierarchy);
            j.Prop("enabled", c is Behaviour b ? b.enabled : (bool?)null);
            j.Prop("collected_by_zone", collected.Contains(shape));
            j.Prop("collected_index", z.Shapes.IndexOf(shape));
            j.Prop("hierarchy_index", shapeIndex);
            j.Key("local");
            FieldDumper.WriteFields(j, c);
            j.Prop("local_to_world", c.transform.localToWorldMatrix);
            var center = shape.Center();
            var bounds = shape.GetBounds();
            j.Prop("world_center", center);
            j.Prop("world_bounds", bounds);
            // A fixed probe lattice over the bounds grown by 1 m: 5 x 3 x 5 points
            // (x, y, z), each with the game's own Contains(point, 0) result.
            j.Arr("probes");
            var min = bounds.min - Vector3.one;
            var size = bounds.size + 2 * Vector3.one;
            for (var ix = 0; ix < 5; ix++)
                for (var iy = 0; iy < 3; iy++)
                    for (var iz = 0; iz < 5; iz++)
                    {
                        var p = new Vector3(min.x + size.x * ix / 4f, min.y + size.y * iy / 2f, min.z + size.z * iz / 4f);
                        j.W.WriteStartArray();
                        j.W.WriteValue(p.x);
                        j.W.WriteValue(p.y);
                        j.W.WriteValue(p.z);
                        j.W.WriteValue(shape.Contains(p));
                        j.W.WriteEndArray();
                    }
            j.W.WriteStartArray();
            j.W.WriteValue(center.x);
            j.W.WriteValue(center.y);
            j.W.WriteValue(center.z);
            j.W.WriteValue(shape.Contains(center));
            j.W.WriteEndArray();
            j.EndArr();
            // The extraction tool's probes for this shape (inventory.json
            // zones[].shapes[].probes, matched by zone unique ID and the shape's index in
            // GetComponentsInChildren(includeInactive) order, as the extractor lists them), each
            // evaluated with the game's Contains(point, corpulence), plus
            // ScriptZone.ContainsPosition(point) (corpulence 0, all collected shapes).
            var list = probes.For(z.UniqueId, shapeIndex);
            if (list != null)
            {
                j.Arr("inventory_probes"); // [x, y, z, corpulence, contains, zone_contains_position, extractor_inside]
                foreach (var pr in list)
                {
                    j.W.WriteStartArray();
                    j.W.WriteValue(pr.Point.x);
                    j.W.WriteValue(pr.Point.y);
                    j.W.WriteValue(pr.Point.z);
                    j.W.WriteValue(pr.Corpulence);
                    j.W.WriteValue(shape.Contains(pr.Point, pr.Corpulence));
                    j.W.WriteValue(z.ContainsPosition(pr.Point));
                    if (pr.Inside.HasValue) j.W.WriteValue(pr.Inside.Value); else j.W.WriteNull();
                    j.W.WriteEndArray();
                }
                j.EndArr();
            }
            j.End();
        }
        j.EndArr();
        j.End();
    }

    private static void WriteObject(Json j, Transform t)
    {
        var go = t.gameObject;
        j.Obj();
        j.Prop("name", go.name);
        j.Prop("path", Paths.Of(t));
        j.Prop("root_sibling_index", t.root.GetSiblingIndex());
        j.Key("sibling_path");
        Paths.WriteSiblingPath(j, t);
        j.Prop("active_self", go.activeSelf);
        j.Prop("active_in_hierarchy", go.activeInHierarchy);
        j.Prop("layer", go.layer);
        j.Prop("static", go.isStatic);
        j.Prop("local_position", t.localPosition);
        j.Prop("local_rotation", t.localRotation);
        j.Prop("local_scale", t.localScale);
        j.Prop("position", t.position);
        j.Prop("rotation", t.rotation);
        j.Prop("lossy_scale", t.lossyScale);
        j.Prop("local_to_world", t.localToWorldMatrix);
        j.Arr("components"); // [type, enabled (null if not a Behaviour/Renderer/Collider)]
        foreach (var c in go.GetComponents<Component>())
        {
            j.W.WriteStartArray();
            j.W.WriteValue(c == null ? "(missing script)" : c.GetType().FullName);
            switch (c)
            {
                case Behaviour b: j.W.WriteValue(b.enabled); break;
                case Renderer rr: j.W.WriteValue(rr.enabled); break;
                case Collider cc: j.W.WriteValue(cc.enabled); break;
                default: j.W.WriteNull(); break;
            }
            j.W.WriteEndArray();
        }
        j.EndArr();
        var mf = go.GetComponent<MeshFilter>();
        if (mf != null && mf.sharedMesh != null) j.Prop("mesh", mf.sharedMesh.name);
        var r = go.GetComponent<Renderer>();
        if (r != null)
        {
            j.Prop("renderer_enabled", r.enabled);
            j.Arr("materials");
            foreach (var m in r.sharedMaterials) j.W.WriteValue(m != null ? m.name + "|" + (m.shader != null ? m.shader.name : "") : null);
            j.EndArr();
        }
        j.End();
    }
}
