using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Runtime.CompilerServices;
using HarmonyLib;
using Kingmaker.TurnBasedMode.Pathfinding;
using Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar;
using Pathfinding;
using TurnBased.Controllers;
using UnityEngine;

namespace WrathOracle;

// Observer "paths" (docs/poc5/paths.md): what turn-based paths are computed on and with, so a replay can compute
// every path and check the grid it searched:
//  - nav_guo: each GraphUpdateObject queued (AstarPath.UpdateGraphs), with its id (one per object: RevertNavmeshForUnit
//    queues the same object again, flipped), bounds (centre and extents), graph mask, flags, and the caller chain;
//  - nav_update: GridGraph.UpdateArea applying one (the object's id and the erodeIterations it erodes with);
//  - nav_tags: CombatController.UpdateNavigationGridTags returning: erodeIterations, erosionOnlyAroundCellsWithTag and
//    the grid's state (FNV-1a 64 over each node, in index order, of flags & 0xFFF80001 as u32 LE, gridFlags as u16 LE,
//    DynamicBlocked and AllyBlocked as bytes; the counts of dynamically and ally-blocked cells);
//  - path_fin: each path returned (Path.ReturnPath, on the main thread) with its key (PathSync's, or the agent observer's
//    id), kind, error, CompleteState, searchedNodes, its end node's grid index, vectorPath before the seeker's modifiers,
//    and the LineOfSightGeometry.HasObstacle calls LosPathConditionForGrid.TargetFound made while it was computed, on
//    whatever thread (from, to, fudge radius, result, in order);
//  - path_seek: Seeker.OnPathComplete, whether the seeker takes the path (its current one) and runs modifiers;
//  - path_mod: the seeker's post-processing done (Seeker.RunModifiers(PostProcess) returning), with the key and the
//    vectorPath its callback gets.
// Gated like the other observers: inside a turn-based window; records are written on the main thread.
internal static partial class CommandObs
{
    internal static bool PathsOn;

    private static bool PathsObserved() => On && PathsOn;

    private sealed class GuoBox { public int Id; }
    private static ConditionalWeakTable<GraphUpdateObject, GuoBox> _guoIds = new ConditionalWeakTable<GraphUpdateObject, GuoBox>();
    private static int _nextGuo;

    private static int GuoId(GraphUpdateObject o)
    {
        if (!_guoIds.TryGetValue(o, out var b))
        {
            b = new GuoBox { Id = ++_nextGuo };
            _guoIds.Add(o, b);
        }
        return b.Id;
    }

    // ---- AstarPath.UpdateGraphs(GraphUpdateObject) ----

    public static void NavGuoPrefix(GraphUpdateObject ob)
    {
        try
        {
            if (!PathsObserved() || ob == null) return;
            var id = GuoId(ob);
            var b = ob.bounds;
            var c = b.center;
            var e = b.extents;
            var mask = ob.nnConstraint != null ? ob.nnConstraint.graphMask.value : (int?)null;
            var flags = new[]
            {
                ob.updatePhysics, ob.updateErosion, ob.modifyWalkability, ob.setWalkability, ob.modifyDynamicWalkability,
                ob.setDynamicWalkability, ob.modifyAllyBlocked, ob.setAllyBlocked, ob.modifyTag,
            };
            var tag = ob.setTag;
            var penalty = ob.addPenalty;
            var shape = ob.shape != null;
            var origin = Observe.Callers(3);
            Sched.Obs("nav_guo", j =>
            {
                j.Prop("id", id); j.Prop("c", c); j.Prop("e", e); j.Prop("mask", mask);
                j.Prop("f", string.Concat(flags.Select(x => x ? '1' : '0')));
                j.Prop("tag", tag); j.Prop("pen", penalty); j.Prop("shape", shape); j.Prop("origin", origin);
            });
        }
        catch (Exception x) { Trace.HookFailed("observe nav_guo", x); }
    }

    // ---- GridGraph.UpdateArea (IUpdatableGraph, explicit) ----

    public static void NavAreaPrefix(GridGraph __instance, GraphUpdateObject o)
    {
        try
        {
            if (!PathsObserved() || o == null) return;
            var id = GuoId(o);
            var erode = o.updateErosion ? __instance.erodeIterations : 0;
            Sched.Obs("nav_update", j => { j.Prop("id", id); j.Prop("erosion", erode); });
        }
        catch (Exception x) { Trace.HookFailed("observe nav_update", x); }
    }

    // ---- CombatController.UpdateNavigationGridTags ----

    private static readonly AccessTools.FieldRef<GridNodeBase, ushort> GridFlagsOf =
        AccessTools.FieldRefAccess<GridNodeBase, ushort>("gridFlags");

    public static void NavTagsPostfix()
    {
        try
        {
            if (!PathsObserved()) return;
            var g = AstarPath.active?.data?.gridGraph;
            if (g == null) { Sched.Obs("nav_tags", j => j.Key("grid").W.WriteNull()); return; }
            ulong h = 0xcbf29ce484222325UL;
            void Byte(byte v) { h ^= v; h *= 0x100000001b3UL; }
            int dyn = 0, ally = 0;
            var nodes = g.nodes;
            for (var i = 0; i < nodes.Length; i++)
            {
                var n = nodes[i];
                var f = n.Flags & 0xFFF80001u;
                Byte((byte)f); Byte((byte)(f >> 8)); Byte((byte)(f >> 16)); Byte((byte)(f >> 24));
                var gf = GridFlagsOf(n);
                Byte((byte)gf); Byte((byte)(gf >> 8));
                Byte(unchecked((byte)n.DynamicBlocked));
                Byte(unchecked((byte)n.AllyBlocked));
                if (n.DynamicBlocked != 0) dyn++;
                if (n.AllyBlocked != 0) ally++;
            }
            var erode = g.erodeIterations;
            var only = g.erosionOnlyAroundCellsWithTag;
            var hash = h.ToString("x16");
            Sched.Obs("nav_tags", j =>
            {
                j.Prop("erode", erode); j.Prop("only", only); j.Prop("hash", hash); j.Prop("dyn", dyn); j.Prop("ally", ally);
            });
        }
        catch (Exception x) { Trace.HookFailed("observe nav_tags", x); }
    }

    // ---- line of sight inside LosPathConditionForGrid.TargetFound (any thread) ----

    [ThreadStatic] private static Path _losPath;

    private static readonly AccessTools.FieldRef<ABPathEndingCondition, ABPath> CondPath =
        AccessTools.FieldRefAccess<ABPathEndingCondition, ABPath>("abPath");

    private struct LosCall { public Vector3 From, To; public float Fudge; public bool Result; }

    // By path object and pathID (A* pools Path objects).
    private static readonly Dictionary<Path, (ushort id, List<LosCall> calls)> LosCalls = new Dictionary<Path, (ushort, List<LosCall>)>();

    public static void LosCondPrefix(ABPathWithClearance.LosPathConditionForGrid __instance)
    {
        try { _losPath = PathsOn ? CondPath(__instance) : null; }
        catch { _losPath = null; }
    }

    public static Exception LosCondFinalizer(Exception __exception)
    {
        _losPath = null;
        return __exception;
    }

    public static void LosPostfix(Vector3 from, Vector3 to, float fudgeRadius, bool __result)
    {
        var p = _losPath;
        if (p == null) return;
        try
        {
            lock (LosCalls)
            {
                if (!LosCalls.TryGetValue(p, out var e) || e.id != p.pathID)
                {
                    e = (p.pathID, new List<LosCall>());
                    LosCalls[p] = e;
                }
                e.calls.Add(new LosCall { From = from, To = to, Fudge = fudgeRadius, Result = __result });
            }
        }
        catch { }
    }

    private static List<LosCall> TakeLos(Path p)
    {
        lock (LosCalls)
        {
            if (!LosCalls.TryGetValue(p, out var e)) return null;
            LosCalls.Remove(p);
            return e.id == p.pathID ? e.calls : null;
        }
    }

    // ---- Path.ReturnPath (main thread), Seeker.RunModifiers(PostProcess) ----

    public static void PathFinPrefix(Path __instance)
    {
        try
        {
            var los = TakeLos(__instance);
            if (!PathsObserved()) return;
            var key = PathRef(__instance);
            var kind = __instance.GetType().Name;
            var err = __instance.error;
            var cs = (int)__instance.CompleteState;
            var searched = Traverse.Create(__instance).Field("searchedNodes").GetValue<int>();
            int? end = null;
            if (__instance is ABPath ab && ab.endNode is GridNodeBase gn) end = gn.NodeInGridIndex;
            var raw = __instance.vectorPath?.ToList();
            Sched.Obs("path_fin", j =>
            {
                j.Prop("path", key); j.Prop("kind", kind); j.Prop("err", err); j.Prop("cs", cs); j.Prop("searched", searched);
                j.Prop("end", end);
                j.Key("raw");
                if (raw == null) j.W.WriteNull();
                else { j.W.WriteStartArray(); foreach (var x in raw) j.Vec(x); j.W.WriteEndArray(); }
                // Every returned path's transcript, empty when its search asked nothing.
                j.Key("los");
                {
                    j.W.WriteStartArray();
                    foreach (var c in los ?? new List<LosCall>())
                    {
                        j.W.WriteStartArray(); j.Vec(c.From); j.Vec(c.To); j.W.WriteValue(c.Fudge); j.W.WriteValue(c.Result);
                        j.W.WriteEndArray();
                    }
                    j.W.WriteEndArray();
                }
            });
        }
        catch (Exception x) { Trace.HookFailed("observe path_fin", x); }
    }

    private static readonly AccessTools.FieldRef<Seeker, Path> SeekerPath = AccessTools.FieldRefAccess<Seeker, Path>("path");

    // Seeker.OnPathComplete(p, runModifiers, sendCallbacks): whether the seeker takes the path (its current one: a path
    // superseded after it was computed returns without modifiers or callbacks).
    public static void PathSeekPrefix(Seeker __instance, Path p, bool runModifiers, bool sendCallbacks)
    {
        try
        {
            if (!PathsObserved() || p == null) return;
            var key = PathRef(p);
            var current = __instance != null && ReferenceEquals(SeekerPath(__instance), p);
            Sched.Obs("path_seek", j =>
            {
                j.Prop("path", key); j.Prop("accepted", current); j.Prop("mods", runModifiers); j.Prop("cb", sendCallbacks);
            });
        }
        catch (Exception x) { Trace.HookFailed("observe path_seek", x); }
    }

    public static void PathModPostfix(Seeker.ModifierPass pass, Path path)
    {
        try
        {
            if (!PathsObserved() || pass != Seeker.ModifierPass.PostProcess || path == null) return;
            var key = PathRef(path);
            var v = path.vectorPath?.ToList();
            Sched.Obs("path_mod", j =>
            {
                j.Prop("path", key);
                j.Key("v");
                if (v == null) j.W.WriteNull();
                else { j.W.WriteStartArray(); foreach (var x in v) j.Vec(x); j.W.WriteEndArray(); }
            });
        }
        catch (Exception x) { Trace.HookFailed("observe path_mod", x); }
    }

    // ---- install ----

    private static void InstallPaths(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        var failed = Health.FailedCount;
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        MethodBase Exact(Type t, string n, params Type[] args)
        {
            var m = t.GetMethod(n, all, null, args, null);
            if (m == null) Health.Failed("paths observe: " + t.Name + "." + n + " missing", new Exception("paths"));
            return m;
        }
        patch(Exact(typeof(AstarPath), nameof(AstarPath.UpdateGraphs), typeof(GraphUpdateObject)), nameof(NavGuoPrefix), null, null);
        patch(Exact(typeof(GridGraph), "Pathfinding.IUpdatableGraph.UpdateArea", typeof(GraphUpdateObject)), nameof(NavAreaPrefix), null, null);
        patch(Exact(typeof(CombatController), "UpdateNavigationGridTags"), null, nameof(NavTagsPostfix), null);
        patch(Exact(typeof(ABPathWithClearance.LosPathConditionForGrid), nameof(ABPathWithClearance.LosPathConditionForGrid.TargetFound), typeof(PathNode)),
            nameof(LosCondPrefix), null, nameof(LosCondFinalizer));
        patch(Exact(typeof(LineOfSightGeometry), nameof(LineOfSightGeometry.HasObstacle), typeof(Vector3), typeof(Vector3), typeof(float)),
            null, nameof(LosPostfix), null);
        patch(Exact(typeof(Path), "ReturnPath"), nameof(PathFinPrefix), null, null);
        patch(Exact(typeof(Seeker), nameof(Seeker.RunModifiers), typeof(Seeker.ModifierPass), typeof(Path)), null, nameof(PathModPostfix), null);
        patch(Exact(typeof(Seeker), "OnPathComplete", typeof(Path), typeof(bool), typeof(bool)), nameof(PathSeekPrefix), null, null);
        PathsOn = Health.FailedCount == failed;
    }

    internal static void ResetPathsWindow()
    {
        _guoIds = new ConditionalWeakTable<GraphUpdateObject, GuoBox>();
        _nextGuo = 0;
        lock (LosCalls) LosCalls.Clear();
    }
}
