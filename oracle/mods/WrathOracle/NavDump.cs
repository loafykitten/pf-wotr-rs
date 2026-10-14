using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using Kingmaker;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.TurnBasedMode.Pathfinding;
using Kingmaker.View;
using Kingmaker.View.MapObjects.SriptZones;
using Kingmaker.View.Spawners;
using Pathfinding;
using IOPath = System.IO.Path;
using Path = Pathfinding.Path;
using UnityEngine;

namespace WrathOracle;

// Navigation dump and queries through the game's own A* Pathfinding Project (main thread).
//   nav-graph.json: every graph's settings; navmesh graphs in full (tiles, vertices,
//     triangles, nodes with tags, flags and connections); grid graphs as packed arrays.
//   nav-queries.json: a fixed list of start/end points in Prologue_Caves_1 (resolved from
//     the live scene by unique ID, with literal fallbacks) and, per query:
//       navmesh_raw   ABPath on the navmesh graph, no modifiers: node path and costs;
//       navmesh_agent the real-time Seeker result: a SeekerWithClearance set up exactly as
//                     UnitMovementAgent.Init does (funnel modifiers, exact start point),
//                     graph mask 1, all tags, as UnitMovementAgent.PathTo requests it;
//       grid_raw      ABPath on the grid graph (turn-based movement's graph), no modifiers;
//       tb_clearance_0.5 / _1.0  ABPathWithClearance on the grid (mask 2) through a seeker
//                     set up as UnitMovementAgent is in turn-based mode.
//     Then the same variants for every query in the extraction tool's nav.json.
//     navmesh_raw is the path before modifiers; navmesh_agent after them.
// Node keys are stable across loads: navmesh [graph, tile index, index in tile.nodes];
// grid [graph, 0, NodeInGridIndex]. A*'s NodeIndex is reassigned on load.
// navmesh_agent's "points" are the Seeker's result; "points_after_remove_excess_points" adds
// the agent's own postprocessing (real time, out of combat).
internal static class NavDump
{
    private sealed class Endpoint
    {
        public string Name, Ref, Source;
        public Vector3 Fallback, Point;
    }

    // Entity unique IDs from docs/poc0/fights.md (scene UUIDs of zones and unit groups);
    // fallbacks are the documented coordinates.
    private static List<Endpoint> Endpoints() => new List<Endpoint>
    {
        new Endpoint { Name = "party", Ref = "party", Fallback = new Vector3(116.82f, 39.30f, 140.93f) },
        new Endpoint { Name = "seelah_anevia_zone", Ref = "e1c57f38-117a-4cdf-b80e-ca4f68271377", Fallback = new Vector3(108.38f, 40.32f, 127.85f) },
        new Endpoint { Name = "camelia_zone", Ref = "f515ee41-9fc6-44bb-97b8-dd145cffd481", Fallback = new Vector3(73.06f, 36.43f, 154.30f) },
        new Endpoint { Name = "first_battle", Ref = "6da882ea-e3df-43f8-a566-daaf2797c538", Fallback = new Vector3(49f, 0f, 155.5f) },
        new Endpoint { Name = "optional1", Ref = "05d04025-637d-4682-9811-1879229ae7d9", Fallback = new Vector3(35f, 0f, 115f) },
        new Endpoint { Name = "optional2", Ref = "a613693c-5857-43ae-8834-7ee8dbe677c2", Fallback = new Vector3(45.5f, 0f, 133.5f) },
        new Endpoint { Name = "optional3", Ref = "5f8c819e-ba22-4362-ae6c-7282198b5c34", Fallback = new Vector3(70.5f, 0f, 119f) },
        new Endpoint { Name = "before_lann", Ref = "14436cf5-b5e5-439e-b2e1-58ee49a9f07a", Fallback = new Vector3(88f, 0f, 110f) },
        new Endpoint { Name = "after_climb", Ref = "a4ac186e-26e6-4b59-b840-70592ea07e1a", Fallback = new Vector3(121.5f, 0f, 76.5f) },
        new Endpoint { Name = "optional4", Ref = "a8eebc18-32de-4693-9c0e-4787f97a485c", Fallback = new Vector3(144f, 0f, 65f) },
        new Endpoint { Name = "attack_ac_damage_tutor", Ref = "2e47134b-2df1-4533-a3ea-71c91a5703a5", Fallback = new Vector3(80.5f, 0f, 67.5f) },
        new Endpoint { Name = "optional5", Ref = "00c54470-15a6-4443-87f8-62d825a888c8", Fallback = new Vector3(58.5f, 0f, 57.5f) },
    };

    private static readonly string[][] Pairs =
    {
        new[] { "party", "seelah_anevia_zone" },
        new[] { "party", "camelia_zone" },
        new[] { "seelah_anevia_zone", "camelia_zone" },
        new[] { "camelia_zone", "first_battle" },
        new[] { "party", "first_battle" },
        new[] { "first_battle", "optional1" },
        new[] { "first_battle", "optional2" },
        new[] { "party", "optional3" },
        new[] { "party", "before_lann" },
        new[] { "before_lann", "after_climb" },
        new[] { "party", "optional4" },
        new[] { "party", "attack_ac_damage_tutor" },
        new[] { "party", "optional5" },
        new[] { "optional5", "after_climb" },
        new[] { "camelia_zone", "party" },
        new[] { "first_battle", "party" },
    };

    // The Caves_1 Arbiter points (requested rig targets), for nearest-node checks only.
    private static readonly Vector3[] RoutePoints = CameraRoute.AuthoredPoints.Select(p => p.Position).ToArray();

    private static readonly FieldInfo FlagsField = typeof(GraphNode).GetField("flags", BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public);
    private static readonly FieldInfo GridFlagsField = typeof(GridNode).GetField("gridFlags", BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public);
    private static readonly FieldInfo SearchedNodesField = typeof(Path).GetField("searchedNodes", BindingFlags.Instance | BindingFlags.NonPublic);

    private static Dictionary<GraphNode, int> _indexInTile;

    // CombatController.IsInTurnBasedCombat() can set CanChangeWeaponSet; mirror its test.
    private static bool InTurnBasedCombat()
    {
        var game = Game.Instance;
        return game.Player.IsInCombat && game.Player.IsTurnBasedModeOn() && game.CurrentMode != Kingmaker.GameModes.GameModeType.Cutscene;
    }

    // A fingerprint of the live graphs' searchable state (FNV-1a 64 over every node's
    // position, raw flags, tag, dynamic/ally blocking and connection costs, in GetNodes
    // order), to detect any change between the snapshot and the end of the queries.
    private static string GraphSignature(AstarPath astar)
    {
        ulong h = 14695981039346656037UL;
        void Mix(long v)
        {
            for (var i = 0; i < 8; i++) { h ^= (byte)(v >> (8 * i)); h *= 1099511628211UL; }
        }
        foreach (var g in astar.data.graphs)
        {
            Mix(g.graphIndex);
            g.GetNodes(n =>
            {
                Mix(n.position.x); Mix(n.position.y); Mix(n.position.z);
                Mix(FlagsField != null ? (uint)FlagsField.GetValue(n) : 0u);
                Mix(n.DynamicBlocked); Mix(n.AllyBlocked);
                if (n is MeshNode mn && mn.connections != null)
                    foreach (var c in mn.connections) { Mix(c.node != null ? c.node.NodeIndex : -1); Mix(c.cost); }
                else if (n is GridNode gn)
                    Mix(GridFlagsField != null ? (ushort)GridFlagsField.GetValue(gn) : 0);
            });
        }
        return h.ToString("x16");
    }

    public static void Run(string outDir, string extractorNavPath)
    {
        var astar = AstarPath.active ?? throw new InvalidOperationException("AstarPath.active is null");
        // Apply pending graph updates and work items first, so the snapshot is the graph the
        // queries run on; then check it didn't change by the end.
        astar.FlushGraphUpdates();
        astar.FlushWorkItems();
        var signatureBefore = GraphSignature(astar);
        _indexInTile = new Dictionary<GraphNode, int>();
        foreach (var g in astar.data.graphs.OfType<NavmeshBase>())
            foreach (var tile in g.GetTiles() ?? new NavmeshTile[0])
                if (tile?.nodes != null)
                    for (var i = 0; i < tile.nodes.Length; i++) _indexInTile[tile.nodes[i]] = i;

        using (var j = new Json(IOPath.Combine(outDir, "nav-graph.json")))
        {
            j.Obj();
            j.Prop("schema", "wrath-oracle-nav-graph-v1");
            j.Prop("int3_precision", Int3.Precision);
            j.Key("random_state_before");
            RandomState.Write(j);
            j.Obj("astar")
                .Prop("heuristic", astar.heuristic.ToString())
                .Prop("heuristic_scale", astar.heuristicScale)
                .Prop("max_nearest_node_distance", astar.maxNearestNodeDistance)
                .Prop("thread_count", astar.threadCount.ToString())
                .Prop("using_multithreading", astar.IsUsingMultithreading)
                .End();
            j.Arr("graphs");
            for (var gi = 0; gi < astar.data.graphs.Length; gi++)
            {
                var g = astar.data.graphs[gi];
                var index = gi;
                j.Record("graph", x => WriteGraph(x, g, index));
            }
            j.EndArr();
            j.Key("random_state_after");
            RandomState.Write(j);
            Health.Write(j);
            j.End();
        }

        var recast = astar.data.graphs.FirstOrDefault(g => g is NavmeshBase);
        var grid = astar.data.graphs.FirstOrDefault(g => g is GridGraph);
        var endpoints = Endpoints();
        foreach (var e in endpoints) Resolve(e);

        using (var j = new Json(IOPath.Combine(outDir, "nav-queries.json")))
        {
            j.Obj();
            j.Prop("schema", "wrath-oracle-nav-queries-v1");
            j.Prop("note", "Paths are computed with AstarPath.BlockUntilCalculated, which also returns any other pending paths (their callbacks run earlier than they otherwise would). Take this dump before timed or observational steps.");
            j.Prop("navmesh_graph_index", recast != null ? (int)recast.graphIndex : -1);
            j.Prop("grid_graph_index", grid != null ? (int)grid.graphIndex : -1);
            j.Prop("in_turn_based_combat", InTurnBasedCombat()); // mirrored: the game's getter has a side effect
            j.Arr("endpoints");
            foreach (var e in endpoints)
                j.Record("endpoint", x =>
                {
                    x.Obj().Prop("name", e.Name).Prop("ref", e.Ref).Prop("source", e.Source).Prop("point", e.Point).Prop("fallback", e.Fallback);
                    WriteNearest(x, "nearest_navmesh", e.Point, recast);
                    WriteNearest(x, "nearest_grid", e.Point, grid);
                    x.End();
                });
            j.EndArr();

            j.Arr("route_points");
            for (var i = 0; i < RoutePoints.Length; i++)
            {
                var idx = i;
                j.Record("route point", x =>
                {
                    x.Obj().Prop("sample", idx).Prop("point", RoutePoints[idx]);
                    WriteNearest(x, "nearest_navmesh", RoutePoints[idx], recast);
                    WriteNearest(x, "nearest_grid", RoutePoints[idx], grid);
                    x.End();
                });
            }
            j.EndArr();

            j.Key("random_state_before");
            RandomState.Write(j);
            var seekerHost = CreateAgentSeeker(out var seeker, turnBased: false);
            var tbHost = CreateAgentSeeker(out var tbSeeker, turnBased: true);
            try
            {
                j.Arr("queries");
                foreach (var pair in Pairs)
                {
                    var a = endpoints.First(e => e.Name == pair[0]);
                    var b = endpoints.First(e => e.Name == pair[1]);
                    j.Record("query", x => WriteQuery(x, a.Name + " -> " + b.Name, a.Point, b.Point, recast, grid, seeker, tbSeeker));
                }
                j.EndArr();

                var extractor = ExtractorQueries.Load(extractorNavPath, out var source);
                j.Prop("extractor_queries_source", source);
                j.Arr("extractor_queries");
                foreach (var q in extractor)
                    j.Record("extractor query", x => WriteQuery(x, q.label, q.start, q.end, recast, grid, seeker, tbSeeker));
                j.EndArr();
            }
            finally
            {
                UnityEngine.Object.Destroy(seekerHost);
                UnityEngine.Object.Destroy(tbHost);
            }
            j.Key("random_state_after");
            RandomState.Write(j);
            var signatureAfter = GraphSignature(astar);
            j.Prop("graph_fingerprint", signatureBefore);
            j.Prop("graph_changed_during_queries", signatureAfter != signatureBefore);
            if (signatureAfter != signatureBefore) Health.Failed("nav", new InvalidOperationException("a graph changed between the snapshot and the end of the queries"));
            Health.Write(j);
            j.End();
        }
        _indexInTile = null;
    }

    // One query, all variants. tb_clearance_*: ABPathWithClearance on the grid graph
    // (mask 2) through a turn-based-configured seeker, clearance 2 x corpulence and the tag
    // mask UnitMovementAgent.ConstructTags builds in turn-based combat for that size.
    private static void WriteQuery(Json x, string label, Vector3 start, Vector3 end, NavGraph recast, NavGraph grid,
        SeekerWithClearance seeker, SeekerWithClearance tbSeeker)
    {
        x.Obj();
        x.Prop("label", label).Prop("start", start).Prop("end", end);
        if (recast != null)
        {
            x.Key("navmesh_raw");
            RunRaw(x, start, end, recast);
            x.Key("navmesh_agent");
            RunAgent(x, seeker, start, end);
        }
        if (grid != null)
        {
            x.Key("grid_raw");
            RunRaw(x, start, end, grid);
            foreach (var corp in new[] { 0.5f, 1.0f })
            {
                x.Key("tb_clearance_" + corp.ToString("0.0", System.Globalization.CultureInfo.InvariantCulture));
                RunTurnBased(x, tbSeeker, start, end, corp, (GridGraph)grid);
            }
        }
        x.End();
    }

    private static int TurnBasedTags(float corpulence, GridGraph grid)
    {
        var size = Mathf.RoundToInt(corpulence / grid.nodeSize);
        if (size < 1) size = 1;
        if (size > grid.erodeIterations) size = grid.erodeIterations;
        var mask = int.MaxValue;
        for (var i = 0; i < size - 1; i++) mask &= ~(1 << (i + grid.erosionFirstTag));
        return mask;
    }

    private static void RunTurnBased(Json j, SeekerWithClearance seeker, Vector3 a, Vector3 b, float corpulence, GridGraph grid)
    {
        Path done = null;
        uint? g = null;
        seeker.clearance = new Vector3(corpulence * 2f, 0f, corpulence * 2f);
        seeker.traversableTags = TurnBasedTags(corpulence, grid);
        var p = ABPathWithClearance.Construct(a, b, seeker.clearance, null, null, false, 0f, float.MaxValue);
        p.immediateCallback = r => g = EndNodeG(r);
        seeker.StartPath(p, r => done = r, 2);
        AstarPath.BlockUntilCalculated(p);
        WritePath(j, done ?? p, done == null ? "callback not invoked" : null, g,
            $"corpulence {corpulence}, clearance {seeker.clearance.x}, tags 0x{seeker.traversableTags:x8}");
        seeker.ReleaseClaimedPath();
    }

    private static readonly FieldInfo PathHandlerField = typeof(Path).GetField("pathHandler", BindingFlags.Instance | BindingFlags.NonPublic);

    // A*'s G of the end node, read in the path's immediate callback (pathfinding thread,
    // after the search, while the path handler still holds this search's node data).
    private static uint? EndNodeG(Path p)
    {
        try
        {
            var end = (p as ABPath)?.endNode;
            var handler = PathHandlerField?.GetValue(p) as PathHandler;
            if (end == null || handler == null || p.CompleteState != PathCompleteState.Complete) return null;
            var node = handler.GetPathNode(end);
            return node.pathID == p.pathID ? node.G : (uint?)null; // only if this search reached it
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static void Resolve(Endpoint e)
    {
        e.Point = e.Fallback;
        e.Source = "fallback";
        var game = Game.Instance;
        if (e.Ref == "party")
        {
            var u = game.Player.MainCharacter.Value;
            if (u != null) { e.Point = u.Position; e.Source = "main character position"; }
            return;
        }
        // Views in the loaded scenes, by unique ID. Zones resolve to the game's own shape
        // centre; unit groups to the mean of their spawners' positions.
        foreach (var s in SceneDump.LoadedScenes())
            foreach (var root in s.GetRootGameObjects())
                foreach (var v in root.GetComponentsInChildren<EntityViewBase>(true))
                {
                    if (v.UniqueId != e.Ref) continue;
                    if (v is ScriptZone z && z.Shapes.Count > 0) { e.Point = z.Shapes[0].Center(); e.Source = "script zone shape centre"; return; }
                    if (v is UnitGroupView g)
                    {
                        var sp = g.GetComponentsInChildren<UnitSpawnerBase>(true).Where(x => x.transform.parent == g.transform).ToList();
                        if (sp.Count > 0)
                        {
                            var sum = Vector3.zero;
                            foreach (var x in sp) sum += x.transform.position;
                            e.Point = sum / sp.Count;
                            e.Source = $"mean of {sp.Count} spawner positions";
                            return;
                        }
                    }
                    e.Point = v.transform.position;
                    e.Source = "view position";
                    return;
                }
    }

    private static GameObject CreateAgentSeeker(out SeekerWithClearance seeker, bool turnBased)
    {
        // As UnitMovementAgent.Init: SeekerWithClearance, a RaycastModifier (graph raycasting;
        // enabled only in turn-based), and two FunnelModifiers (m_FunnelModifier, enabled
        // when not turn-based, plus a second one that stays enabled), exact start point.
        var go = new GameObject("WrathOracle nav probe") { hideFlags = HideFlags.DontSave };
        seeker = go.AddComponent<SeekerWithClearance>();
        var smooth = go.AddComponent<RaycastModifier>();
        smooth.useRaycasting = false;
        smooth.useGraphRaycasting = true;
        var funnel = go.AddComponent<FunnelModifier>();
        go.AddComponent<FunnelModifier>();
        seeker.startEndModifier.exactStartPoint = StartEndModifier.Exactness.Original;
        var tb = turnBased; // the agent's m_SmoothModifier/m_FunnelModifier as in turn-based (true) or real time (false)
        smooth.enabled = tb;
        funnel.enabled = !tb;
        seeker.traversableTags = -1;
        seeker.drawGizmos = false;
        var u = Game.Instance.Player.MainCharacter.Value;
        if (u != null) seeker.clearance = new Vector3(u.Corpulence * 2f, 0f, u.Corpulence * 2f);
        return go;
    }

    private static void RunRaw(Json j, Vector3 a, Vector3 b, NavGraph graph)
    {
        uint? g = null;
        var p = ABPath.Construct(a, b);
        p.nnConstraint.graphMask = GraphMask.FromGraph(graph);
        p.immediateCallback = r => g = EndNodeG(r);
        AstarPath.StartPath(p);
        AstarPath.BlockUntilCalculated(p);
        WritePath(j, p, null, g);
    }

    private static void RunAgent(Json j, Seeker seeker, Vector3 a, Vector3 b)
    {
        Path done = null;
        uint? g = null;
        seeker.traversableTags = -1;
        var p = ABPath.Construct(a, b);
        p.immediateCallback = r => g = EndNodeG(r);
        seeker.StartPath(p, r => done = r, 1); // graph mask 1, as UnitMovementAgent.PathTo in real time
        AstarPath.BlockUntilCalculated(p);
        // points: the Seeker's result (StartEnd + funnel modifiers). The agent then runs
        // PathPostprocess.RemoveExcessPoints on it out of combat in real time (and, in
        // real-time combat, ObstaclePathfinder.PathAroundStandingObstacles, not run here).
        var result = done ?? p;
        List<Vector3> after = null;
        if (!result.error && result.vectorPath != null)
        {
            var saved = new List<Vector3>(result.vectorPath);
            PathPostprocess.RemoveExcessPoints(result);
            after = new List<Vector3>(result.vectorPath);
            result.vectorPath.Clear();
            result.vectorPath.AddRange(saved);
        }
        WritePath(j, result, done == null ? "callback not invoked" : null, g, "seeker result before agent postprocessing", after);
        seeker.ReleaseClaimedPath();
    }

    private static void WritePath(Json j, Path p, string note, uint? endG, string config = null, List<Vector3> postprocessed = null)
    {
        j.Obj();
        if (note != null) j.Prop("note", note);
        if (config != null) j.Prop("config", config);
        if (endG.HasValue) j.Prop("end_node_g", endG.Value); else j.Null("end_node_g");
        j.Prop("state", p.CompleteState.ToString());
        j.Prop("error", p.errorLog);
        j.Prop("duration_ms", p.duration);
        j.Prop("searched_nodes", SearchedNodesField != null ? (int)SearchedNodesField.GetValue(p) : -1);
        if (p is ABPath ab)
        {
            j.Prop("start_point", ab.startPoint).Prop("end_point", ab.endPoint);
            j.Prop("original_start", ab.originalStartPoint).Prop("original_end", ab.originalEndPoint);
            j.Key("start_node"); NodeKey(j, ab.startNode);
            j.Key("end_node"); NodeKey(j, ab.endNode);
        }
        var nodes = p.path ?? new List<GraphNode>();
        j.Prop("node_count", nodes.Count);
        j.Arr("nodes");
        foreach (var n in nodes) NodeKey(j, n);
        j.EndArr();
        // Cost of each step = the stored connection cost from node i to node i+1; the sum
        // excludes A*'s endpoint costs (start/end point to node).
        j.Arr("step_costs");
        long sum = 0;
        for (var i = 0; i + 1 < nodes.Count; i++)
        {
            var c = ConnectionCost(nodes[i], nodes[i + 1]);
            sum += c < 0 ? 0 : c;
            j.W.WriteValue(c);
        }
        j.EndArr();
        j.Prop("connection_cost_sum", sum);
        var pts = p.vectorPath ?? new List<Vector3>();
        j.Arr("points");
        foreach (var v in pts) j.Vec(v);
        j.EndArr();
        j.Prop("length", p.GetTotalLength());
        if (postprocessed != null)
        {
            j.Arr("points_after_remove_excess_points");
            foreach (var v in postprocessed) j.Vec(v);
            j.EndArr();
        }
        j.End();
    }

    private static long ConnectionCost(GraphNode from, GraphNode to)
    {
        long cost = -1;
        if (from is MeshNode mn && mn.connections != null)
            foreach (var c in mn.connections)
                if (c.node == to) return c.cost;
        if (from is GridNodeBase gn && to is GridNodeBase)
        {
            var graph = GridNode.GetGridGraph(gn.GraphIndex);
            for (var dir = 0; dir < 8; dir++)
                if (gn.GetNeighbourAlongDirection(dir) == to) return graph.neighbourCosts[dir];
        }
        return cost;
    }

    private static readonly System.Reflection.FieldInfo GraphTransformMatrix =
        typeof(Pathfinding.Util.GraphTransform).GetField("matrix", BindingFlags.Instance | BindingFlags.NonPublic);

    private static Matrix4x4 GraphMatrix(Pathfinding.Util.GraphTransform t) =>
        t != null && GraphTransformMatrix != null ? (Matrix4x4)GraphTransformMatrix.GetValue(t) : Matrix4x4.identity;

    public static void NodeKey(Json j, GraphNode n)
    {
        if (n == null) { j.W.WriteNull(); return; }
        j.W.WriteStartArray();
        j.W.WriteValue(n.GraphIndex);
        switch (n)
        {
            case TriangleMeshNode t:
                j.W.WriteValue((t.v0 >> NavmeshBase.TileIndexOffset) & NavmeshBase.TileIndexMask);
                j.W.WriteValue(_indexInTile != null && _indexInTile.TryGetValue(t, out var i) ? i : -1);
                break;
            case GridNodeBase g:
                j.W.WriteValue(0);
                j.W.WriteValue(g.NodeInGridIndex);
                break;
            default:
                j.W.WriteValue(-1);
                j.W.WriteValue(n.NodeIndex);
                break;
        }
        j.W.WriteEndArray();
    }

    private static void WriteNearest(Json j, string key, Vector3 p, NavGraph graph)
    {
        j.Obj(key);
        if (graph == null) { j.End(); return; }
        var c = NNConstraint.Default;
        c.graphMask = GraphMask.FromGraph(graph);
        var nn = AstarPath.active.GetNearest(p, c);
        j.Key("node"); NodeKey(j, nn.node);
        j.Prop("position", nn.position);
        j.Prop("distance", Vector3.Distance(p, nn.position));
        j.End();
    }

    private static void WriteGraph(Json j, NavGraph g, int index)
    {
        j.Obj();
        j.Prop("index", index);
        j.Prop("graph_index", g.graphIndex);
        j.Prop("type", g.GetType().FullName);
        j.Prop("name", g.name);
        var count = 0;
        g.GetNodes(_ => count++);
        j.Prop("node_count", count);
        j.Obj("settings");
        foreach (var f in g.GetType().GetFields(BindingFlags.Instance | BindingFlags.Public))
        {
            var t = f.FieldType;
            if (!(t.IsPrimitive || t.IsEnum || t == typeof(string) || t == typeof(Vector3) || t == typeof(Vector2) || t == typeof(Quaternion) || t == typeof(Int3))) continue;
            var v = f.GetValue(g);
            j.Key(f.Name);
            switch (v)
            {
                case Vector3 v3: j.Vec(v3); break;
                case Vector2 v2: j.W.WriteStartArray(); j.W.WriteValue(v2.x); j.W.WriteValue(v2.y); j.W.WriteEndArray(); break;
                case Quaternion q: j.Quat(q); break;
                case Int3 i3: j.W.WriteStartArray(); j.W.WriteValue(i3.x); j.W.WriteValue(i3.y); j.W.WriteValue(i3.z); j.W.WriteEndArray(); break;
                case Enum e: j.W.WriteValue(e.ToString()); break;
                default: j.W.WriteValue(v); break;
            }
        }
        j.End();

        if (g is NavmeshBase nm) WriteNavmesh(j, nm);
        else if (g is GridGraph gg) WriteGrid(j, gg);
        j.End();
    }

    private static void WriteNavmesh(Json j, NavmeshBase g)
    {
        j.Prop("tile_x_count", g.tileXCount);
        j.Prop("tile_z_count", g.tileZCount);
        j.Prop("transform", GraphMatrix(g.transform));
        j.Arr("tiles");
        var tiles = g.GetTiles() ?? new NavmeshTile[0];
        for (var ti = 0; ti < tiles.Length; ti++)
        {
            var tile = tiles[ti];
            if (tile == null) { j.W.WriteNull(); continue; }
            j.Obj();
            j.Prop("array_index", ti);
            j.Prop("x", tile.x).Prop("z", tile.z).Prop("w", tile.w).Prop("d", tile.d);
            j.Arr("verts"); // world space, Int3 (millimetres)
            foreach (var v in tile.verts) { j.W.WriteValue(v.x); j.W.WriteValue(v.y); j.W.WriteValue(v.z); }
            j.EndArr();
            j.Arr("tris");
            foreach (var t in tile.tris) j.W.WriteValue(t);
            j.EndArr();
            // nodes[i]: [vertex index 0..2 (within tile), flags, tag, walkable, penalty,
            //   [tile, node, cost, shape edge]...]
            j.Arr("nodes");
            foreach (var n in tile.nodes)
            {
                j.W.WriteStartArray();
                j.W.WriteValue(n.v0 & NavmeshBase.VertexIndexMask);
                j.W.WriteValue(n.v1 & NavmeshBase.VertexIndexMask);
                j.W.WriteValue(n.v2 & NavmeshBase.VertexIndexMask);
                j.W.WriteValue(FlagsField != null ? (uint)FlagsField.GetValue(n) : 0u);
                j.W.WriteValue(n.Tag);
                j.W.WriteValue(n.Walkable);
                j.W.WriteValue(n.Penalty);
                j.W.WriteStartArray();
                if (n.connections != null)
                    foreach (var c in n.connections)
                    {
                        if (c.node is TriangleMeshNode o)
                        {
                            j.W.WriteValue((o.v0 >> NavmeshBase.TileIndexOffset) & NavmeshBase.TileIndexMask);
                            j.W.WriteValue(_indexInTile.TryGetValue(o, out var oi) ? oi : -1);
                        }
                        else
                        {
                            j.W.WriteValue(-1);
                            j.W.WriteValue(c.node != null ? c.node.NodeIndex : -1);
                        }
                        j.W.WriteValue(c.cost);
                        j.W.WriteValue(c.shapeEdge);
                    }
                j.W.WriteEndArray();
                j.W.WriteEndArray();
            }
            j.EndArr();
            j.End();
        }
        j.EndArr();
    }

    private static void WriteGrid(Json j, GridGraph g)
    {
        j.Prop("width", g.width).Prop("depth", g.depth).Prop("node_size", g.nodeSize);
        j.Prop("center", g.center).Prop("rotation", g.rotation);
        j.Prop("erode_iterations", g.erodeIterations);
        j.Prop("transform", GraphMatrix(g.transform));
        j.Arr("neighbour_costs");
        foreach (var c in g.neighbourCosts) j.W.WriteValue(c);
        j.EndArr();
        j.Arr("neighbour_offsets");
        foreach (var c in g.neighbourOffsets) j.W.WriteValue(c);
        j.EndArr();
        // Packed little-endian arrays indexed by NodeInGridIndex (null nodes are zeros):
        // positions Int3 (3 x int32), flags uint32, grid_flags uint16 (bits 0-7 connections).
        var n = g.width * g.depth;
        var pos = new byte[n * 12];
        var flags = new byte[n * 4];
        var gflags = new byte[n * 2];
        var dyn = new byte[n * 2]; // DynamicBlocked, AllyBlocked (sbyte each)
        var effective = new byte[n];
        var nodes = g.nodes;
        for (var i = 0; nodes != null && i < nodes.Length && i < n; i++)
        {
            var node = nodes[i];
            if (node == null) continue;
            Buffer.BlockCopy(BitConverter.GetBytes(node.position.x), 0, pos, i * 12, 4);
            Buffer.BlockCopy(BitConverter.GetBytes(node.position.y), 0, pos, i * 12 + 4, 4);
            Buffer.BlockCopy(BitConverter.GetBytes(node.position.z), 0, pos, i * 12 + 8, 4);
            Buffer.BlockCopy(BitConverter.GetBytes(FlagsField != null ? (uint)FlagsField.GetValue(node) : 0u), 0, flags, i * 4, 4);
            Buffer.BlockCopy(BitConverter.GetBytes(GridFlagsField != null ? (ushort)GridFlagsField.GetValue(node) : (ushort)0), 0, gflags, i * 2, 2);
            dyn[i * 2] = unchecked((byte)node.DynamicBlocked);
            dyn[i * 2 + 1] = unchecked((byte)node.AllyBlocked);
            effective[i] = (byte)(node.Walkable ? 1 : 0);
        }
        j.Prop("positions_int3_le_base64", Convert.ToBase64String(pos));
        j.Prop("flags_u32_le_base64", Convert.ToBase64String(flags));
        j.Prop("grid_flags_u16_le_base64", Convert.ToBase64String(gflags));
        j.Prop("dynamic_ally_blocked_i8x2_base64", Convert.ToBase64String(dyn));
        j.Prop("walkable_effective_u8_base64", Convert.ToBase64String(effective));
        j.Prop("walkable_count", nodes?.Count(x => x != null && x.Walkable) ?? 0);
    }
}
