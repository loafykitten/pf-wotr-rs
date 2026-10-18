using System;
using System.Collections.Generic;
using System.Reflection.Emit;
using System.Reflection;
using HarmonyLib;
using Kingmaker.Controllers.Optimization;
using Kingmaker.EntitySystem;
using Kingmaker.View;
using UnityEngine;

namespace WrathOracle;

// Observer "bounds_init" (declared with the memory observer and bounds_tick's hook; docs/poc5/vision.md, the window's start): at the window's
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
// Observer "bounds_tick" (with bounds_init): each EntityBoundsController.TickEntity call (the bounds controller's update
// of one entity: every entity at its first tick and after an area part change, else the update list's), so a replay
// takes the colliders' radii and positions as the game set them instead of inferring them from corpulence reads:
//  - e: the entity's UniqueId; kind: its type's name;
//  - made: whether the call created the collider; r0/r: the radius before (null without a collider) and after;
//  - pos: the collider transform's position after (Unity's; PhysX gets it at the next sync); on: enabled after;
//  - c: the collider's GameObject instance id (bounds_init's id) or null;
//  - x: the exception's type when TickEntity threw (TickEntitySafe catches and logs it), else absent. A throw between
//    SetupSphereBoundsCollider's creation of the collider and its assignment to the view leaves a collider in the scene
//    that no record names (made false, r null): a replay must refuse a run with x.
// Observer "bounds_set" (with bounds_tick): the collider writes outside TickEntity, by EntityViewBase:
//  - fn: "active" (UpdateViewActive, which sets enabled to ShouldBeEnumeratedByEntityPoolEnumerator; written only
//    when the flag changed), "pos" (UpdateSphereColliderPositionIfNeeded: camp places, Alushinyrra's isles; only when
//    the position changed), "gone" (DestroySphereBoundsCollider: Object.Destroy, so the collider leaves the physics
//    scene at the frame's end, not at once), "link" (SetupSphereColliderEntityLink: the collider's EntityDataLink
//    relinked to the view's new data, or the entity queued for a bounds update when the view has no collider);
//  - v: the view's data's UniqueId (m_Data) or null; c: the collider's GameObject instance id or null;
//  - on0/on, pos0/pos, e0/e: enabled, the transform's position and the linked entity's UniqueId before and after;
//  - x: the exception's type when the method threw.
//  Also fn "entity": EntityDataLink.Entity's setter, wherever it's called from (SetupSphereBoundsCollider's link,
//  SetupSphereColliderEntityLink, EntityDataLink.OnDisable's unlink when the collider's GameObject is disabled or
//  destroyed): c the link's GameObject, e0/e the entity before and after, v null, on/pos the link's own collider.
// Observer "bounds_scene" (with bounds_tick): EntityBoundsController.Tick found its scene invalid and made a new one
// (at its first tick, or after the scene was unloaded: the main menu's cleanup does that after resetting the game, and
// a debug cheat does it in play, destroying every collider without a record): a replay must refuse a run with one
// after the window's start.
// Observer "phys_sync" (with bounds_tick): each Physics.SyncTransforms() the game calls (which flushes every changed
// transform to PhysX, the bounds scene's included, outside the fixed steps): src names the caller
// (Polymorph.RestoreView, UnitPlaceOnGroundController.Tick's forced place, GameActionSetIsleState.RunAction,
// IsleStateControllerView.ResetAllIslands), and "DollRoom.Update" for DollRoom's PhysicsScene.Simulate (made only when
// Physics.autoSimulation is off: the player's simulate flushes every pending transform the same way, the bounds
// scene's included). A transpiler replaces each call with a tap that writes the record and then makes the call, so a
// record is exactly a sync (the callers' conditions included).
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
    internal static bool BoundsOn;

    private struct BoundsBefore { public bool Had; public float R; }

    private static void InstallBounds(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        var failed = Health.FailedCount;
        var m = AccessTools.Method(typeof(EntityBoundsController), "TickEntity");
        if (m == null) { Health.Failed("bounds observe: EntityBoundsController.TickEntity missing", new Exception("TickEntity")); return; }
        patch(m, nameof(BoundsTickPrefix), null, nameof(BoundsTickFinalizer));
        foreach (var name in new[] { "UpdateViewActive", "UpdateSphereColliderPositionIfNeeded", "DestroySphereBoundsCollider", "SetupSphereColliderEntityLink" })
        {
            var v = AccessTools.Method(typeof(EntityViewBase), name);
            if (v == null) { Health.Failed("bounds observe: EntityViewBase." + name + " missing", new Exception(name)); continue; }
            patch(v, nameof(BoundsSetPrefix), null, nameof(BoundsSetFinalizer));
        }
        var set = AccessTools.PropertySetter(typeof(EntityDataLink), nameof(EntityDataLink.Entity));
        if (set == null) Health.Failed("bounds observe: EntityDataLink.Entity's setter missing", new Exception("set_Entity"));
        else patch(set, nameof(BoundsLinkPrefix), null, nameof(BoundsLinkFinalizer));
        var tick = AccessTools.Method(typeof(EntityBoundsController), nameof(EntityBoundsController.Tick));
        if (tick == null) Health.Failed("bounds observe: EntityBoundsController.Tick missing", new Exception("Tick"));
        else patch(tick, nameof(BoundsSceneTickPrefix), null, null);
        InstallSyncTaps(h);
        BoundsOn = Health.FailedCount == failed;
    }

    private static void BoundsTickPrefix(EntityDataBase entity, out BoundsBefore __state)
    {
        __state = default;
        if (!On || !BoundsOn) return;
        try
        {
            var sc = entity?.View != null ? entity.View.SphereBoundsCollider : null;
            if (sc != null) { __state.Had = true; __state.R = sc.radius; }
        }
        catch (Exception e) { Trace.HookFailed("observe bounds_tick", e); }
    }

    private static Exception BoundsTickFinalizer(Exception __exception, EntityDataBase entity, BoundsBefore __state)
    {
        if (!On || !BoundsOn) return __exception;
        try
        {
            var id = entity?.UniqueId;
            var kind = entity?.GetType().Name;
            var sc = entity?.View != null ? entity.View.SphereBoundsCollider : null;
            var had = __state.Had;
            var r0 = __state.R;
            float? r = sc != null ? sc.radius : (float?)null;
            bool? on = sc != null ? sc.enabled : (bool?)null;
            Vector3? pos = sc != null ? sc.transform.position : (Vector3?)null;
            int? c = sc != null ? sc.gameObject.GetInstanceID() : (int?)null;
            var x = __exception?.GetType().Name;
            Sched.Obs("bounds_tick", j =>
            {
                j.Prop("e", id); j.Prop("kind", kind);
                j.Prop("made", !had && sc != null);
                NullableFloat(j, "r0", had ? r0 : (float?)null); NullableFloat(j, "r", r);
                NullableVec(j, "pos", pos);
                j.Key("on"); if (on.HasValue) j.W.WriteValue(on.Value); else j.W.WriteNull();
                j.Key("c"); if (c.HasValue) j.W.WriteValue(c.Value); else j.W.WriteNull();
                if (x != null) j.Prop("x", x);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe bounds_tick", e); }
        return __exception;
    }
    private struct ColliderState { public bool Had; public int C; public bool On; public Vector3 Pos; public string E; }

    private static readonly AccessTools.FieldRef<EntityViewBase, EntityDataBase> ViewDataField =
        AccessTools.FieldRefAccess<EntityViewBase, EntityDataBase>("m_Data");

    private static ColliderState ColliderOf(EntityViewBase v)
    {
        var st = default(ColliderState);
        var sc = v != null ? v.SphereBoundsCollider : null;
        if (sc == null) return st;
        st.Had = true; st.C = sc.gameObject.GetInstanceID(); st.On = sc.enabled; st.Pos = sc.transform.position;
        var link = sc.GetComponent<EntityDataLink>();
        st.E = link != null ? LinkEntityField(link)?.UniqueId : null;
        return st;
    }

    private static void BoundsSetPrefix(EntityViewBase __instance, out ColliderState __state)
    {
        __state = default;
        if (!On || !BoundsOn) return;
        try { __state = ColliderOf(__instance); }
        catch (Exception e) { Trace.HookFailed("observe bounds_set", e); }
    }

    private static Exception BoundsSetFinalizer(Exception __exception, EntityViewBase __instance, MethodBase __originalMethod, ColliderState __state)
    {
        if (!On || !BoundsOn) return __exception;
        try
        {
            var fn = __originalMethod.Name switch
            {
                "UpdateViewActive" => "active",
                "UpdateSphereColliderPositionIfNeeded" => "pos",
                "DestroySphereBoundsCollider" => "gone",
                _ => "link",
            };
            var b = __state;
            var a = ColliderOf(__instance);
            var x = __exception?.GetType().Name;
            // UpdateViewActive and the position update run often: written only when they changed something.
            var changed = b.Had != a.Had || b.C != a.C || b.On != a.On || b.Pos != a.Pos || b.E != a.E;
            if (!changed && x == null && (fn == "active" || fn == "pos")) return __exception;
            if (fn == "gone" && !b.Had && x == null) return __exception;
            var v = ViewDataField(__instance)?.UniqueId;
            Sched.Obs("bounds_set", j =>
            {
                j.Prop("fn", fn); j.Prop("v", v);
                j.Key("c"); if (b.Had || a.Had) j.W.WriteValue(b.Had ? b.C : a.C); else j.W.WriteNull();
                j.Key("on0"); if (b.Had) j.W.WriteValue(b.On); else j.W.WriteNull();
                j.Key("on"); if (a.Had) j.W.WriteValue(a.On); else j.W.WriteNull();
                NullableVec(j, "pos0", b.Had ? b.Pos : (Vector3?)null); NullableVec(j, "pos", a.Had ? a.Pos : (Vector3?)null);
                j.Prop("e0", b.E); j.Prop("e", a.E);
                if (x != null) j.Prop("x", x);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe bounds_set", e); }
        return __exception;
    }
    private static void BoundsLinkPrefix(EntityDataLink __instance, out string __state)
    {
        __state = null;
        if (!On || !BoundsOn) return;
        try { __state = LinkEntityField(__instance)?.UniqueId; }
        catch (Exception e) { Trace.HookFailed("observe bounds_set entity", e); }
    }

    private static Exception BoundsLinkFinalizer(Exception __exception, EntityDataLink __instance, string __state)
    {
        if (!On || !BoundsOn) return __exception;
        try
        {
            var e0 = __state;
            var e1 = LinkEntityField(__instance)?.UniqueId;
            var c = __instance.gameObject.GetInstanceID();
            var sc = __instance.GetComponent<SphereCollider>();
            bool? on = sc != null ? sc.enabled : (bool?)null;
            Vector3? pos = sc != null ? sc.transform.position : (Vector3?)null;
            var x = __exception?.GetType().Name;
            Sched.Obs("bounds_set", j =>
            {
                j.Prop("fn", "entity"); j.Prop("v", (string)null); j.Prop("c", c);
                j.Key("on0"); j.W.WriteNull();
                j.Key("on"); if (on.HasValue) j.W.WriteValue(on.Value); else j.W.WriteNull();
                j.Key("pos0"); j.W.WriteNull(); NullableVec(j, "pos", pos);
                j.Prop("e0", e0); j.Prop("e", e1);
                if (x != null) j.Prop("x", x);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe bounds_set entity", e); }
        return __exception;
    }

    private static void BoundsSceneTickPrefix(EntityBoundsController __instance)
    {
        if (!On || !BoundsOn) return;
        try
        {
            if (!__instance.Scene.IsValid()) Sched.Obs("bounds_scene");
        }
        catch (Exception e) { Trace.HookFailed("observe bounds_scene", e); }
    }
    private static readonly MethodInfo SyncTransformsMethod = AccessTools.Method(typeof(Physics), nameof(Physics.SyncTransforms));

    private static void SyncTap(string src)
    {
        if (On && BoundsOn)
        {
            try { Sched.Obs("phys_sync", j => j.Prop("src", src)); }
            catch (Exception e) { Trace.HookFailed("observe phys_sync", e); }
        }
        Physics.SyncTransforms();
    }

    public static void SyncTapPolymorph() => SyncTap("Polymorph.RestoreView");
    public static void SyncTapPlace() => SyncTap("UnitPlaceOnGroundController.Tick");
    public static void SyncTapIsleAction() => SyncTap("GameActionSetIsleState.RunAction");
    public static void SyncTapIsleReset() => SyncTap("IsleStateControllerView.ResetAllIslands");

    private static readonly MethodInfo SimulateMethod =
        AccessTools.Method(typeof(PhysicsScene), nameof(PhysicsScene.Simulate), new[] { typeof(float) });

    // DollRoom's m_PhysicsScene.Simulate(dt): the call's receiver is the field's address.
    public static void SyncTapDoll(ref PhysicsScene scene, float step)
    {
        if (On && BoundsOn)
        {
            try { Sched.Obs("phys_sync", j => j.Prop("src", "DollRoom.Update")); }
            catch (Exception e) { Trace.HookFailed("observe phys_sync", e); }
        }
        scene.Simulate(step);
    }

    private static readonly Dictionary<IntPtr, string> SyncTaps = new Dictionary<IntPtr, string>();

    public static IEnumerable<CodeInstruction> SyncTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        SyncTaps.TryGetValue(original.MethodHandle.Value, out var tap);
        foreach (var i in instructions)
        {
            var target = tap == nameof(SyncTapDoll) ? SimulateMethod : SyncTransformsMethod;
            if (tap != null && i.opcode == OpCodes.Call && i.operand is MethodInfo mi && mi == target)
            {
                i.operand = AccessTools.Method(typeof(CommandObs), tap);
                var k = "sync:" + tap;
                Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
            }
            yield return i;
        }
    }

    private static void InstallSyncTaps(Harmony h)
    {
        var targets = new (MethodBase m, string tap)[]
        {
            (AccessTools.Method(typeof(Kingmaker.UnitLogic.Buffs.Polymorph), "RestoreView"), nameof(SyncTapPolymorph)),
            (AccessTools.Method(typeof(Kingmaker.Controllers.Units.UnitPlaceOnGroundController), "Tick"), nameof(SyncTapPlace)),
            (AccessTools.Method(typeof(Kingmaker.ElementsSystem.GameActionSetIsleState), "RunAction"), nameof(SyncTapIsleAction)),
            (AccessTools.Method(typeof(Locations.Alushenyrra.IsleStateControllerView), "ResetAllIslands"), nameof(SyncTapIsleReset)),
            (AccessTools.Method(typeof(Kingmaker.UI.ServiceWindow.DollRoom), "Update", new[] { typeof(bool) }), nameof(SyncTapDoll)),
        };
        if (SimulateMethod == null || SimulateMethod.ReturnType != typeof(void))
            Health.Failed("bounds observe: PhysicsScene.Simulate(float) isn't the void method the doll tap replaces", new Exception("Simulate"));
        foreach (var (m, tap) in targets)
        {
            if (m == null) { Health.Failed("bounds observe: a SyncTransforms caller is missing", new Exception(tap)); continue; }
            SyncTaps[m.MethodHandle.Value] = tap;
            try { h.Patch(m, transpiler: new HarmonyMethod(typeof(CommandObs), nameof(SyncTapTranspiler))); }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name + " (sync tap)", e); continue; }
            var k = "sync:" + tap;
            if (!Tapped.TryGetValue(k, out var n) || n != 1)
                Health.Failed("bounds observe: tap " + k + " replaced " + (Tapped.TryGetValue(k, out var x) ? x : 0) + " calls, not 1", new Exception(k));
        }
    }
}
