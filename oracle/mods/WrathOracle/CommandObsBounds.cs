using System;
using HarmonyLib;
using Kingmaker.Controllers.Optimization;
using Kingmaker.EntitySystem;
using Kingmaker.View;
using UnityEngine;

namespace WrathOracle;

// Observer "bounds_init" (declared with the memory observer; docs/poc5/vision.md, the window's start): at the window's
// first tick, the entity-bounds scene's colliders (EntityBoundsController's SphereColliders, which
// UnitVisionRangeController's overlap queries), so a replay can check its hypotheses about their history. One
// bounds_init per root GameObject of EntityBoundsController.Scene, in GetRootGameObjects order, then bounds_init_end
// with the count:
//  - i: the index in that list: the observed root order. The stock code creates each sphere and moves it to the scene
//    at once (SetupSphereBoundsCollider), which appends it, and nothing else in the game reparents or reorders them,
//    but Unity doesn't promise that the root list is creation order, a destroyed collider leaves no entry, and a
//    disabled and re-enabled collider can register with PhysX again without moving here: evidence, not history;
//  - id: the GameObject's GetInstanceID; layer: its layer;
//  - e: the linked entity's UniqueId (EntityDataLink.m_Entity, read as a field) or null; kind: its type's name;
//  - r: the SphereCollider's radius; on: its enabled flag;
//  - pos: its transform's position. Unity's, not PhysX's: transforms reach PhysX only at a sync (autoSyncTransforms
//    is off; the simulation step syncs), so a collider moved since the last sync is queried at its older position,
//    which isn't readable passively (SyncTransforms would change the game);
//  - fade: the entity view's m_IsFadeHide (UpdateViewActive returns early while it holds), or null without a view.
// Reads only: fields and the collider's, transform's and scene's own getters (GetInstanceID caches only the engine's
// field offset); nothing calls game logic, queries physics or syncs transforms.
internal static partial class CommandObs
{
    private static readonly AccessTools.FieldRef<EntityDataLink, EntityDataBase> LinkEntityField =
        AccessTools.FieldRefAccess<EntityDataLink, EntityDataBase>("m_Entity");
    private static readonly AccessTools.FieldRef<EntityViewBase, bool> FadeHideField =
        AccessTools.FieldRefAccess<EntityViewBase, bool>("m_IsFadeHide");

    private static void BoundsInit()
    {
        try
        {
            var scene = Kingmaker.Game.Instance.EntityBoundsController.Scene;
            var n = 0;
            if (scene.IsValid())
            {
                foreach (var go in scene.GetRootGameObjects())
                {
                    var i = n++;
                    var id = go.GetInstanceID();
                    var layer = go.layer;
                    var sc = go.GetComponent<SphereCollider>();
                    var link = go.GetComponent<EntityDataLink>();
                    var e = link != null ? LinkEntityField(link) : null;
                    var view = e?.View;
                    bool? fade = view != null ? FadeHideField(view) : (bool?)null;
                    float? r = sc != null ? sc.radius : (float?)null;
                    bool? on = sc != null ? sc.enabled : (bool?)null;
                    var pos = go.transform.position;
                    Sched.Obs("bounds_init", j =>
                    {
                        j.Prop("i", i); j.Prop("id", id); j.Prop("layer", layer);
                        j.Prop("e", e?.UniqueId); j.Prop("kind", e?.GetType().Name);
                        NullableFloat(j, "r", r);
                        j.Key("on"); if (on.HasValue) j.W.WriteValue(on.Value); else j.W.WriteNull();
                        j.Prop("pos", pos);
                        j.Key("fade"); if (fade.HasValue) j.W.WriteValue(fade.Value); else j.W.WriteNull();
                    });
                }
            }
            Sched.Obs("bounds_init_end", j => { j.Prop("n", n); j.Prop("valid", scene.IsValid()); });
        }
        catch (Exception e) { Trace.HookFailed("observe bounds_init", e); }
    }
}
