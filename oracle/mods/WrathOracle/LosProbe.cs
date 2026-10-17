using System;
using System.Collections;
using System.Collections.Generic;
using System.Reflection;
using System.Threading;
using HarmonyLib;
using Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar;
using UnityEngine;

namespace WrathOracle;

// Observer "los_geom" (docs/poc5/vision.md, "Observer changes requested"): the line-of-sight geometry the sim
// imports from the scenes' FogOfWarBlockers (crates/wr-los), as the game holds it, and the game's own answers on
// segments across it, so the sim checks the grid, the transformed points and HasObstacle bit for bit. Run once,
// right after a turn-based fixture's begin record (record and replays alike; reads only):
//  - los_geom: LineOfSightGeometry.Instance's m_Cells (nx, nz), m_Bounds.min (x, z) and every cell's segments
//    (segs: [i, j, A.x, A.y, B.x, B.y, Height], cells in index order, segments in each cell's list order), under its
//    read lock;
//  - los_probe: v: [from.x, from.y, from.z, to.x, to.y, to.z, fudge, 0 or 1], HasObstacle(from, to, fudge) for
//    Probes segments crossing each distinct segment of the geometry at points drawn by System.Random (a fixed seed:
//    the same list every run; UnityEngine.Random is untouched), at eye heights from 1 m below to 1 m above the
//    segment's Height, a quarter of them ending within 0.01 m of the segment's line and a quarter crossing within
//    0.01 m of one of its end points, a third with a fudge radius. HasObstacle's own state is its thread-static
//    cell list, cleared at each call; the paths observer records only calls inside a path search (CommandObsPaths).
internal static class LosProbe
{
    private const int Probes = 40;
    private const int Seed = 20261017;

    internal static bool Installed = true;

    private static readonly Type Cell = AccessTools.Inner(typeof(LineOfSightGeometry), "GeometryCacheCell");
    private static readonly Type Seg = AccessTools.Inner(typeof(LineOfSightGeometry), "LineSegment");

    internal static void Run()
    {
        if (!Installed) return;
        try
        {
            var g = LineOfSightGeometry.Instance;
            var lk = (ReaderWriterLockSlim)AccessTools.Field(typeof(LineOfSightGeometry), "m_Lock").GetValue(g);
            var cells = (Array)AccessTools.Field(typeof(LineOfSightGeometry), "m_Cells").GetValue(g);
            var bounds = (Bounds)AccessTools.Field(typeof(LineOfSightGeometry), "m_Bounds").GetValue(g);
            var lines = AccessTools.Field(Cell, "Lines");
            var fa = AccessTools.Field(Seg, "A");
            var fb = AccessTools.Field(Seg, "B");
            var fh = AccessTools.Field(Seg, "Height");
            var segs = new List<(int i, int j, Vector2 a, Vector2 b, float h)>();
            var nx = 0;
            var nz = 0;
            lk.EnterReadLock();
            try
            {
                if (cells != null)
                {
                    nx = cells.GetLength(0);
                    nz = cells.GetLength(1);
                    for (var i = 0; i < nx; i++)
                    for (var j = 0; j < nz; j++)
                    {
                        var c = cells.GetValue(i, j);
                        if (c == null) continue;
                        foreach (var s in (IList)lines.GetValue(c))
                            segs.Add((i, j, (Vector2)fa.GetValue(s), (Vector2)fb.GetValue(s), (float)fh.GetValue(s)));
                    }
                }
            }
            finally { lk.ExitReadLock(); }
            var min = bounds.min;
            Sched.Obs("los_geom", j =>
            {
                j.Prop("nx", nx); j.Prop("nz", nz);
                j.Arr("min"); j.W.WriteValue(min.x); j.W.WriteValue(min.z); j.EndArr();
                j.Arr("segs");
                foreach (var s in segs)
                {
                    j.Arr(); j.W.WriteValue(s.i); j.W.WriteValue(s.j);
                    j.W.WriteValue(s.a.x); j.W.WriteValue(s.a.y); j.W.WriteValue(s.b.x); j.W.WriteValue(s.b.y); j.W.WriteValue(s.h);
                    j.EndArr();
                }
                j.EndArr();
            });
            var probes = Probe(g, segs);
            Sched.Obs("los_probe", j =>
            {
                j.Arr("v");
                foreach (var p in probes)
                {
                    j.Arr();
                    j.W.WriteValue(p.from.x); j.W.WriteValue(p.from.y); j.W.WriteValue(p.from.z);
                    j.W.WriteValue(p.to.x); j.W.WriteValue(p.to.y); j.W.WriteValue(p.to.z);
                    j.W.WriteValue(p.fudge); j.W.WriteValue(p.v ? 1 : 0);
                    j.EndArr();
                }
                j.EndArr();
            });
        }
        catch (Exception e) { Trace.HookFailed("los probe", e); }
    }

    private static List<(Vector3 from, Vector3 to, float fudge, bool v)> Probe(
        LineOfSightGeometry g, List<(int i, int j, Vector2 a, Vector2 b, float h)> segs)
    {
        var rng = new System.Random(Seed);
        float R(float lo, float hi) => lo + (float)rng.NextDouble() * (hi - lo);
        var seen = new HashSet<(Vector2, Vector2, float)>();
        var probes = new List<(Vector3, Vector3, float, bool)>();
        foreach (var s in segs)
        {
            if (!seen.Add((s.a, s.b, s.h))) continue;
            var d = s.b - s.a;
            if (d.sqrMagnitude == 0f) continue;
            var n = new Vector2(-d.y, d.x).normalized;
            for (var k = 0; k < Probes; k++)
            {
                // Where it crosses: anywhere along the segment, or (every fourth, from the second) within 0.01 m of an end.
                var len = d.magnitude;
                var t = k % 4 != 1 ? R(0f, 1f) : rng.Next(2) == 0 ? R(-0.01f, 0.01f) / len : 1f + R(-0.01f, 0.01f) / len;
                var p = s.a + d * t;
                // Its ends: one on each side (or, every fourth from the third, the far end within 0.01 m of the line).
                var u0 = R(0.5f, 6f);
                var u1 = k % 4 == 2 ? R(-0.01f, 0.01f) : -R(0.5f, 6f);
                var y = s.h + R(-1f, 1f);
                var from = new Vector3(p.x + n.x * u0, y, p.y + n.y * u0);
                var to = new Vector3(p.x + n.x * u1, y + R(-1f, 1f), p.y + n.y * u1);
                if (rng.Next(2) == 0) (from, to) = (to, from);
                var fudge = k % 3 == 0 ? R(0f, 1f) : 0f;
                probes.Add((from, to, fudge, g.HasObstacle(from, to, fudge)));
            }
        }
        return probes;
    }
}
