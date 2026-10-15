using System;
using System.Collections.Generic;
using System.Text;
using UnityEngine;

namespace WrathOracle;

// Observer "phys_probe" (docs/poc5/physics.md, "Observer changes requested"): the colliders the
// physics queries see and a probe of the ones no recorded query reaches, so the sim's physics
// (crates/wr-phys) can verify boxes and triangle meshes as it verified the terrain. Run once,
// right after a turn-based fixture's begin record (record and replays alike: the probe only reads):
//  - ph_settings: Physics.queriesHitTriggers, queriesHitBackfaces, autoSyncTransforms;
//  - ph_col: every Collider in the loaded scenes, inactive ones included: identity (scene, hierarchy
//    path, sibling path, type, instance id), the GameObject's layer and activity, enabled, isTrigger,
//    bounds, the transform's position, rotation, lossyScale and localToWorldMatrix, and per type:
//    box center and size; mesh sharedMesh name, convex, cookingOptions; terrain data name, size and
//    heightmap resolution; capsule/sphere/controller center, radius, height, direction;
//  - ph_probe: rays straight down (100 m, mask Layers.WalkableMask) over a 9 x 9 grid on the xz bounds
//    of each enabled, active box and mesh collider on a walkable layer, from 2 m above the bounds' top:
//    origin, hit, distance, point and the collider hit (identity as in ph_col).
internal static class PhysProbe
{
    internal const int WalkableMask = 2099457;
    private const int Grid = 9;

    internal static bool Installed = true;

    internal static void Run()
    {
        if (!Installed) return;
        try
        {
            Sched.Obs("ph_settings", j =>
            {
                j.Prop("hit_triggers", Physics.queriesHitTriggers);
                j.Prop("hit_backfaces", Physics.queriesHitBackfaces);
                j.Prop("auto_sync", Physics.autoSyncTransforms);
            });
            var all = UnityEngine.Object.FindObjectsOfType<Collider>(true);
            var probed = new List<Collider>();
            foreach (var c in all)
            {
                var go = c.gameObject;
                var t = c.transform;
                var b = c.bounds;
                Sched.Obs("ph_col", j =>
                {
                    WriteId(j, c);
                    j.Prop("layer", go.layer); j.Prop("active", go.activeInHierarchy); j.Prop("enabled", c.enabled);
                    j.Prop("trigger", c.isTrigger); j.Prop("bounds", b);
                    j.Prop("pos", t.position); j.Prop("rot", t.rotation); j.Prop("lossy", t.lossyScale);
                    j.Prop("l2w", t.localToWorldMatrix);
                    switch (c)
                    {
                        case BoxCollider bc: j.Prop("center", bc.center); j.Prop("size", bc.size); break;
                        case MeshCollider mc:
                            j.Prop("mesh", mc.sharedMesh != null ? mc.sharedMesh.name : null);
                            j.Prop("convex", mc.convex); j.Prop("cooking", (int)mc.cookingOptions); break;
                        case TerrainCollider tc:
                            var td = tc.terrainData;
                            j.Prop("terrain", td != null ? td.name : null);
                            if (td != null) { j.Prop("tsize", td.size); j.Prop("tres", td.heightmapResolution); }
                            break;
                        case CapsuleCollider cc:
                            j.Prop("center", cc.center); j.Prop("radius", cc.radius); j.Prop("height", cc.height); j.Prop("dir", cc.direction); break;
                        case SphereCollider sc: j.Prop("center", sc.center); j.Prop("radius", sc.radius); break;
                        case CharacterController ch:
                            j.Prop("center", ch.center); j.Prop("radius", ch.radius); j.Prop("height", ch.height); break;
                    }
                });
                if ((c is BoxCollider || c is MeshCollider) && c.enabled && go.activeInHierarchy && ((1 << go.layer) & WalkableMask) != 0)
                    probed.Add(c);
            }
            foreach (var c in probed)
            {
                var b = c.bounds;
                for (var ix = 0; ix < Grid; ix++)
                for (var iz = 0; iz < Grid; iz++)
                {
                    var o = new Vector3(
                        b.min.x + b.size.x * ix / (Grid - 1),
                        b.max.y + 2f,
                        b.min.z + b.size.z * iz / (Grid - 1));
                    var hit = Physics.Raycast(o, Vector3.down, out var h, 100f, WalkableMask);
                    var d = h.distance;
                    var pt = h.point;
                    var col = h.collider;
                    Sched.Obs("ph_probe", j =>
                    {
                        j.Prop("o", o); j.Prop("hit", hit);
                        if (hit) { j.Prop("dist", d); j.Prop("pt", pt); j.Key("col"); j.W.WriteStartObject(); WriteId(j, col); j.W.WriteEndObject(); }
                    });
                }
            }
        }
        catch (Exception e) { Health.Failed("phys probe", e); }
    }

    // The collider's identity: scene, hierarchy path, sibling path, type, instance id.
    internal static void WriteId(Json j, Collider c)
    {
        if (c == null) { j.Prop("type", (string)null); return; }
        var t = c.transform;
        var names = new List<string>();
        var sib = new List<int>();
        for (var x = t; x != null; x = x.parent) { names.Add(x.name); if (x.parent != null) sib.Add(x.GetSiblingIndex()); }
        names.Reverse(); sib.Reverse();
        var path = new StringBuilder();
        for (var i = 0; i < names.Count; i++) { if (i > 0) path.Append('/'); path.Append(names[i]); }
        j.Prop("scene", c.gameObject.scene.name); j.Prop("path", path.ToString());
        j.Arr("sib"); foreach (var s in sib) j.W.WriteValue(s); j.EndArr();
        j.Prop("type", c.GetType().Name); j.Prop("iid", c.GetInstanceID());
    }
}
