using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Runtime.CompilerServices;
using HarmonyLib;
using Kingmaker.Controllers.Units;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Commands.Base;
using Kingmaker.Utility;
using Kingmaker.View;
using Kingmaker.Visual.Animation.Kingmaker;
using Pathfinding;
using Pathfinding.Util;
using UnityEngine;

namespace WrathOracle;

// Observer "agent" (docs/poc5/movement.md): the movement agent, so a replay can run UnitMovementAgent and
// UnitMoveController itself and take as inputs only when paths arrive, what they hold, physics and a few gates:
//  - ag_tick / ag_tick_end: each UnitMovementAgent.TickMovement call, its delta, the agent's state before and after
//    (its fields, read as fields), its result;
//  - ag_v: a leaf the agent's code reads (by a transpiler on the call: the animation manager's IsPreventingMovement,
//    the view's IsCommandsPreventMovement, MaxSpeed, FlyHeight, the cutscene's control of the unit, the gamepad
//    setting), with the agent's unit; in UnitMoveController.TickUnit, the unit state's CanMove and CanRotate;
//  - ag_phys: each physics query in UnitMovementAgentBase.Move (Physics.Raycast, PhysicUtils.HeuristicDiscCastDown)
//    with its arguments and result, and the context: the tick ("tick", with the unit), ScorePath's simulation ("sim")
//    or another caller ("other");
//  - ag_req / ag_req_end: PathTo and FindPath, their arguments, the state before and after, and the request made
//    (RequestedPath's PathSync key and kind);
//  - ag_done / ag_done_end: OnPathComplete(p, forcedPath) with the path (PathSync key, kind, vectorPath, error,
//    CompleteState) and the state after;
//  - ag_call / ag_call_end: Stop, CompleteMovement, InterruptMovement, SetWaypoint, ForcePath and
//    FollowPrecomputedPath, with the caller chain (origin) and the state after;
//  - ag_ctl / ag_ctl_end: UnitMoveController.TickUnit, the unit's position before; its position and orientation after;
//  - ag_v gmem: ConnectToGroups' first Group read (Memory != null), the evaluation that decides its early return;
//  - ag_cb / ag_cb_end: the agent's calls into the view (OnMovementInterrupted with the destination,
//    OnMovementComplete, OnMovementWaypointUpdate with the index);
//  - ag_rot: UnitEntityData.UpdateSlowRotation's inputs (m_Orientation, DesiredOrientation, maxAngle);
//  - ag_ground: UnitPlaceOnGroundController.TickUnit (physics: an input for now), the position before and after;
//  - ag_pathdef: a path's definition the first time the window refers to it (PathSync key, or an observer id for a path
//    PathSync didn't see), with its points, error and CompleteState, so paths carried into the window are known;
//  - ag_v isdone: each Path.IsDone() the agent evaluates (PathTo, FindPath, IsReallyMoving, PathFailed), with the path.
// Gated like the other observers: inside a turn-based window, on the main thread.
internal static partial class CommandObs
{
    internal static bool AgOn;

    // The agent whose TickMovement (or ScorePath's SimulateTickMovementForTurnBased) runs, innermost last.
    private static readonly List<(UnitMovementAgent a, string ctx)> AgCtx = new List<(UnitMovementAgent, string)>();

    private static readonly Type Ag = typeof(UnitMovementAgent);
    private static readonly Type AgBase = typeof(UnitMovementAgentBase);

    private static AccessTools.FieldRef<UnitMovementAgent, T> AgF<T>(string name) => AccessTools.FieldRefAccess<UnitMovementAgent, T>(name);
    private static AccessTools.FieldRef<UnitMovementAgentBase, T> AgB<T>(string name) => AccessTools.FieldRefAccess<UnitMovementAgentBase, T>(name);

    private static readonly AccessTools.FieldRef<UnitMovementAgent, Path> AgPath = AgF<Path>("m_Path");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Path> AgRequested = AgF<Path>("m_RequestedPath");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Path> AgRequestedFind = AgF<Path>("m_RequestedFindPath");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgRequestedNew = AgF<bool>("m_RequestedNewPath");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgWarmup = AgF<float>("m_WarmupTime");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgRoaming = AgF<bool>("m_Roaming");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, int> AgNextIndex = AgF<int>("m_NextPointIndex");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgRemaining = AgF<float>("m_RemainingPathDistance");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgEta = AgF<float>("EstimatedTimeLeft");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Vector2> AgNextWp = AgF<Vector2>("m_NextWaypoint");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Vector2> AgNextNextWp = AgF<Vector2>("m_NextNextWaypoint");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Vector2> AgNextDir = AgF<Vector2>("m_NextDirection");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Vector3?> AgDest = AgF<Vector3?>("m_Destination");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Vector3?> AgNoPathDest = AgF<Vector3?>("m_NoPathDestination");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgLastTurn = AgF<float>("m_LastTurnAngle");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, int> AgPathFrame = AgF<int>("m_PathFrame");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, int> AgRepathAfter = AgF<int>("m_RepathAfterWaypoint");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgStuckStop = AgF<float>("m_StuckTimeStop");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgStuckDir = AgF<float>("m_StuckTimeDirection");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgMinWp = AgF<float>("m_MinWaypointDistance");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float> AgSlowDown = AgF<float>("m_SlowDownTime");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgFirstTick = AgF<bool>("m_FirstTick");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgForce = AgF<bool>("m_IsInForceMode");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgFollow = AgF<bool>("m_FollowPrecomputedPath");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgNoMoreTb = AgF<bool>("m_NoMoreTurnBasedMovementForUinterruptableCommand");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, int> AgCharging = AgF<int>("m_ChargingCounter");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, float?> AgSavedMinSpeed = AgF<float?>("m_SavedMinSpeed");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, Vector3> AgNextVel = AgF<Vector3>("m_NextVelocity");
    private static readonly AccessTools.FieldRef<UnitMovementAgent, bool> AgAvoidOff = AgF<bool>("m_AvoidanceDisabled");
    private static readonly AccessTools.FieldRef<UnitMovementAgentBase, Vector3> AgVel = AgB<Vector3>("m_Velocity");
    private static readonly AccessTools.FieldRef<UnitMovementAgentBase, float> AgSpeed = AgB<float>("m_Speed");
    private static readonly AccessTools.FieldRef<UnitMovementAgentBase, float> AgMinSpeed = AgB<float>("m_MinSpeed");
    private static readonly AccessTools.FieldRef<UnitMovementAgentBase, float> AgAccel = AgB<float>("m_Acceleration");
    private static readonly AccessTools.FieldRef<UnitMovementAgentBase, float> AgCombatAngular = AgB<float>("m_CombatAngularSpeed");
    private static readonly AccessTools.FieldRef<UnitMovementAgentBase, UnitMovementAgentBase> AgDestAgent = AgB<UnitMovementAgentBase>("m_DestinationAgent");

    // The agent's state, read through fields and auto-properties only.
    private sealed class AgState
    {
        public Vector3 Pos, Vel, NextVel;
        public string Path, Req, Find, DestAgent, DestView;
        public bool ReqNew, Roaming, ForceRoaming, DestHas, DestAttached, Connected, ObstGroup, GroupMemory, Prone, FirstTick, Force, Follow, NoMoreTb, Repath, Stopping, AvoidOff, NavObstacles;
        public float Warmup, Remaining, Eta, LastTurn, StuckStop, StuckDir, MinWp, SlowDown, Speed, MinSpeed, Accel, Angular, CombatAngular, Approach, MaxApproach;
        public float? SavedMinSpeed, MaxSpeedOverride;
        public int NextIndex, PathFrame, RepathAfter, Charging;
        public Vector2 NextWp, NextNextWp, NextDir, MoveDir;
        public Vector3? Dest, NoPathDest;
    }

    private static AgState Snap(UnitMovementAgent a)
    {
        return new AgState
        {
            Pos = a.transform.position,
            Vel = AgVel(a),
            NextVel = AgNextVel(a),
            Path = PathRef(AgPath(a)),
            Req = PathRef(AgRequested(a)),
            Find = PathRef(AgRequestedFind(a)),
            DestAgent = UnitOf(AgDestAgent(a)),
            DestView = DestViewId(AgDestAgent(a), out var attached),
            DestAttached = attached,
            ForceRoaming = a.ForceRoaming,
            Connected = a.ConnectedToObstacles,
            ObstGroup = a.ObstaclesGroup != null,
            GroupMemory = GroupMemoryOf(a),
            DestHas = AgDestAgent(a) != null,
            Prone = a.Unit != null && a.Unit.IsProne,
            ReqNew = AgRequestedNew(a),
            Roaming = AgRoaming(a),
            FirstTick = AgFirstTick(a),
            Force = AgForce(a),
            Follow = AgFollow(a),
            NoMoreTb = AgNoMoreTb(a),
            Repath = a.RepathNeeded,
            Stopping = a.IsStopping,
            AvoidOff = AgAvoidOff(a),
            NavObstacles = ObstacleAnalyzer.HasNavmeshObstacles,
            Warmup = AgWarmup(a),
            Remaining = AgRemaining(a),
            Eta = AgEta(a),
            LastTurn = AgLastTurn(a),
            StuckStop = AgStuckStop(a),
            StuckDir = AgStuckDir(a),
            MinWp = AgMinWp(a),
            SlowDown = AgSlowDown(a),
            Speed = AgSpeed(a),
            MinSpeed = AgMinSpeed(a),
            Accel = AgAccel(a),
            Angular = a.m_AngularSpeed,
            CombatAngular = AgCombatAngular(a),
            Approach = a.ApproachRadius,
            MaxApproach = a.MaxApproachRadius,
            SavedMinSpeed = AgSavedMinSpeed(a),
            MaxSpeedOverride = a.MaxSpeedOverride,
            NextIndex = AgNextIndex(a),
            PathFrame = AgPathFrame(a),
            RepathAfter = AgRepathAfter(a),
            Charging = AgCharging(a),
            NextWp = AgNextWp(a),
            NextNextWp = AgNextNextWp(a),
            NextDir = AgNextDir(a),
            MoveDir = a.MoveDirection,
            Dest = AgDest(a),
            NoPathDest = AgNoPathDest(a),
        };
    }

    private static void WriteState(Json j, AgState s)
    {
        j.Obj("s");
        j.Prop("pos", s.Pos); j.Prop("vel", s.Vel); j.Prop("nvel", s.NextVel);
        j.Prop("path", s.Path); j.Prop("req", s.Req); j.Prop("find", s.Find); j.Prop("dagent", s.DestAgent); j.Prop("dview", s.DestView); j.Prop("dattached", s.DestAttached);
        j.Prop("reqnew", s.ReqNew); j.Prop("roam", s.Roaming); j.Prop("froam", s.ForceRoaming); j.Prop("conn", s.Connected); j.Prop("prone", s.Prone);
        j.Prop("ogroup", s.ObstGroup); j.Prop("gmem", s.GroupMemory); j.Prop("dhas", s.DestHas); j.Prop("first", s.FirstTick); j.Prop("force", s.Force);
        j.Prop("follow", s.Follow); j.Prop("nomoretb", s.NoMoreTb); j.Prop("repath", s.Repath); j.Prop("stopping", s.Stopping);
        j.Prop("avoidoff", s.AvoidOff); j.Prop("navobst", s.NavObstacles);
        j.Prop("warmup", s.Warmup); j.Prop("remain", s.Remaining); j.Prop("eta", s.Eta); j.Prop("lastturn", s.LastTurn);
        j.Prop("stuckstop", s.StuckStop); j.Prop("stuckdir", s.StuckDir); j.Prop("minwp", s.MinWp); j.Prop("slow", s.SlowDown);
        j.Prop("speed", s.Speed); j.Prop("minspeed", s.MinSpeed); j.Prop("accel", s.Accel); j.Prop("ang", s.Angular);
        j.Prop("cang", s.CombatAngular); j.Prop("ar", s.Approach); j.Prop("mar", s.MaxApproach);
        NullableFloat(j, "savedmin", s.SavedMinSpeed); NullableFloat(j, "maxover", s.MaxSpeedOverride);
        j.Prop("idx", s.NextIndex); j.Prop("pframe", s.PathFrame); j.Prop("repathafter", s.RepathAfter); j.Prop("charging", s.Charging);
        j.Prop("wp", s.NextWp); j.Prop("wp2", s.NextNextWp); j.Prop("ndir", s.NextDir); j.Prop("mdir", s.MoveDir);
        NullableVec(j, "dest", s.Dest); NullableVec(j, "nopath", s.NoPathDest);
        j.W.WriteEndObject();
    }

    private static void NullableFloat(Json j, string k, float? v)
    {
        j.Key(k);
        if (v.HasValue) j.W.WriteValue(v.Value); else j.W.WriteNull();
    }

    private static void NullableVec(Json j, string k, Vector3? v)
    {
        j.Key(k);
        if (v.HasValue) j.Vec(v.Value); else j.W.WriteNull();
    }

    // A path by its PathSync request key ("<requester>#<n>"), or "?<kind>#<n>" for one PathSync didn't see (a forced or
    // precomputed path made without AstarPath.StartPath: every directly constructed ForcedPath has pathID 0, so these get
    // an observer id per object and pathID). The first reference in the window writes the path's definition
    // (ag_pathdef). Call outside a record's body.
    private sealed class PathBox { public ushort Pid; public int Id; }
    private static ConditionalWeakTable<Path, PathBox> _pathIds = new ConditionalWeakTable<Path, PathBox>();
    private static int _nextPath;
    private static readonly HashSet<string> PathDefined = new HashSet<string>();
    private static readonly HashSet<string> PathPending = new HashSet<string>();

    private static string PathRef(Path p)
    {
        if (p == null) return null;
        var key = PathSync.KeyOf(p);
        if (key == null)
        {
            if (!_pathIds.TryGetValue(p, out var b) || b.Pid != p.pathID)
            {
                if (b != null) _pathIds.Remove(p);
                b = new PathBox { Pid = p.pathID, Id = ++_nextPath };
                _pathIds.Add(p, b);
            }
            key = "?" + p.GetType().Name + "#" + b.Id;
        }
        // A path's points are the worker's while it computes them: they're copied only once it has handed the path back
        // (PipelineState past Processing) or for one never queued (Created); a pending path is noted once, and defined
        // at a later reference.
        var state = p.PipelineState;
        var safe = state == PathState.Created || state > PathState.Processing;
        if (AgObserved() && !safe && !PathDefined.Contains(key) && PathPending.Add(key))
        {
            var pk = p.GetType().Name;
            Sched.Obs("ag_pathdef", j => { j.Prop("path", key); j.Prop("kind", pk); j.Prop("pending", true); });
        }
        if (AgObserved() && safe && PathDefined.Add(key))
        {
            var kind = p.GetType().Name;
            var pts = p.vectorPath?.ToList();
            var err = p.error;
            var cs = (int)p.CompleteState;
            var pid = (int)p.pathID;
            Sched.Obs("ag_pathdef", j =>
            {
                j.Prop("path", key); j.Prop("kind", kind); j.Prop("pid", pid); j.Prop("err", err); j.Prop("cs", cs);
                j.Arr("v"); if (pts != null) foreach (var x in pts) j.Vec(x); j.EndArr();
            });
        }
        return key;
    }

    // Whether the unit's group has a memory (ConnectToGroups' early return): m_Group read as a field (the Group getter
    // calls UpdateGroup).
    private static readonly AccessTools.FieldRef<UnitEntityData, Kingmaker.UnitLogic.Groups.UnitGroup> GroupField =
        AccessTools.FieldRefAccess<UnitEntityData, Kingmaker.UnitLogic.Groups.UnitGroup>("m_Group");

    private static bool GroupMemoryOf(UnitMovementAgentBase a)
    {
        var data = a.Unit != null ? a.Unit.EntityData : null;
        return data != null && GroupField(data)?.Memory != null;
    }

    // The destination agent's view's UniqueId (a field), and whether the view still has its data.
    private static string DestViewId(UnitMovementAgentBase a, out bool attached)
    {
        attached = false;
        if (a == null || a.Unit == null) return null;
        attached = a.Unit.EntityData != null;
        return a.Unit.UniqueId;
    }

    private static string UnitOfAgent(UnitMovementAgent a)
    {
        try { return a != null && a.Unit != null ? a.Unit.EntityData?.UniqueId : null; }
        catch (Exception e) { Trace.HookFailed("observe agent unit", e); return null; }
    }

    private static bool AgObserved() => On && AgOn;

    // ---- TickMovement ----

    public static void AgTickPrefix(UnitMovementAgent __instance, float deltaTime)
    {
        AgCtx.Add((__instance, "tick"));
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var s = Snap(__instance);
            // The turn state CanMoveInTurnBased reads (pure getters), for checking the sim's.
            var data = __instance.Unit != null ? __instance.Unit.EntityData : null;
            var combat = data != null && data.IsInCombat;
            var tb = global::TurnBased.Controllers.CombatController.IsInTurnBasedCombat();
            var turn = Kingmaker.Game.Instance.TurnBasedCombatController?.CurrentTurn;
            var cur = data != null && data.IsCurrentUnit();
            var acting = turn != null && turn.IsActing;
            // CanMoveInTurnBased's `Commands.Move?.DontInterruptInTurnBasedMode` (a field; Move is a pure getter).
            var move = data?.Commands?.Move;
            bool? dontint = move == null ? (bool?)null : move.DontInterruptInTurnBasedMode;
            Sched.Obs("ag_tick", j =>
            {
                j.Prop("u", u); j.Prop("dt", deltaTime); j.Prop("combat", combat); j.Prop("tb", tb); j.Prop("turn", turn != null);
                j.Prop("cur", cur); j.Prop("acting", acting); j.Prop("dontint", dontint); WriteState(j, s);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_tick", e); }
    }

    public static void AgTickPostfix(UnitMovementAgent __instance, Vector3 __result)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var s = Snap(__instance);
            Sched.Obs("ag_tick_end", j => { j.Prop("u", u); j.Prop("ret", __result); WriteState(j, s); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_tick_end", e); }
    }

    public static void AgCtxFinalizer()
    {
        if (AgCtx.Count > 0) AgCtx.RemoveAt(AgCtx.Count - 1);
    }

    public static void AgSimPrefix(UnitMovementAgent __instance) => AgCtx.Add((__instance, "sim"));

    // CanMoveInTurnBased(ref deltaTime): its answer and the delta it leaves.
    public static void AgCanMovePostfix(UnitMovementAgent __instance, float deltaTime, bool __result)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            Sched.Obs("ag_v", j => { j.Prop("f", "canmovetb"); j.Prop("u", u); j.Prop("v", __result); j.Prop("dt", deltaTime); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_v canmovetb", e); }
    }

    // ---- leaves ----

    private static string AgUnit() => AgCtx.Count == 0 ? null : UnitOfAgent(AgCtx[AgCtx.Count - 1].a);

    private static void AgLeaf(string f, Action<Json> v)
    {
        try
        {
            if (!AgObserved()) return;
            var u = AgUnit();
            Sched.Obs("ag_v", j => { j.Prop("f", f); j.Prop("u", u); v(j); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_v " + f, e); }
    }

    public static bool TapAgAnimPrevents(UnitAnimationManager m)
    {
        var v = m.IsPreventingMovement;
        AgLeaf("animprev", j => j.Prop("v", v));
        return v;
    }

    public static bool TapAgCommandsPrevent(UnitEntityView view)
    {
        var v = view.IsCommandsPreventMovement;
        AgLeaf("cmdprev", j => j.Prop("v", v));
        return v;
    }

    public static float TapAgMaxSpeed(UnitMovementAgentBase a)
    {
        var v = a.MaxSpeed;
        AgLeaf("maxspeed", j => j.Prop("v", v));
        return v;
    }

    public static float TapAgFlyHeight(UnitEntityData u)
    {
        var v = u.FlyHeight;
        AgLeaf("fly", j => j.Prop("v", v));
        return v;
    }

    public static Kingmaker.AreaLogic.Cutscenes.CutsceneControlledUnit TapAgCutsceneControl(UnitEntityData u)
    {
        var v = u.CutsceneControlledUnit;
        var b = v != null;
        AgLeaf("csctl", j => j.Prop("v", b));
        return v;
    }

    public static bool TapAgGamepad(Kingmaker.Game g)
    {
        var v = g.IsControllerGamepad;
        AgLeaf("gamepad", j => j.Prop("v", v));
        return v;
    }

    public static bool TapAgIsDone(Path p)
    {
        var v = p.IsDone();
        try
        {
            if (AgObserved())
            {
                var key = PathRef(p);
                AgLeaf("isdone", j => { j.Prop("path", key); j.Prop("v", v); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe ag_v isdone", e); }
        return v;
    }

    public static Kingmaker.AreaLogic.Cutscenes.Cutscene TapAgCutsceneActive(Kingmaker.AreaLogic.Cutscenes.CutsceneControlledUnit c)
    {
        var v = c.GetCurrentlyActive();
        var b = v != null;
        AgLeaf("csactive", j => j.Prop("v", b));
        return v;
    }

    public static CapsuleCollider TapAgSoftCollider(UnitEntityView view)
    {
        var v = view.SoftCollider;
        var b = v != null;
        AgLeaf("softcol", j => j.Prop("v", b));
        return v;
    }

    public static Vector3 TapAgClosestPoint(Collider c, Vector3 position)
    {
        var v = c.ClosestPoint(position);
        AgLeaf("closest", j => { j.Prop("p", position); j.Prop("v", v); });
        return v;
    }

    public static Kingmaker.EntitySystem.EntityDataBase TapAgProxyEntity(Kingmaker.EntitySystem.EntityService.Proxy proxy)
    {
        var v = proxy.Entity;
        var id = v is UnitEntityData ud ? ud.UniqueId : null;
        var unit = v is UnitEntityData;
        // What TickMovement makes of it: `unitEntityData.View.Or(null)?.AgentASP` (auto-properties).
        var view = (v as UnitEntityData)?.View;
        var agent = view != null ? view.AgentASP : null;
        var agentView = agent != null && agent.Unit != null ? agent.Unit.UniqueId : null;
        var hasAgent = agent != null;
        AgLeaf("proxy", j => { j.Prop("unit", unit); j.Prop("v", id); j.Prop("agent", hasAgent); j.Prop("aview", agentView); });
        return v;
    }

    // UnitMoveController.TickUnit's reads of the unit's state.
    private static UnitEntityData _ctlUnit;

    public static bool TapCtlCanMove(UnitState s)
    {
        var v = s.CanMove;
        CtlLeaf("canmove", v);
        return v;
    }

    public static bool TapCtlCanRotate(UnitState s)
    {
        var v = s.CanRotate;
        CtlLeaf("canrotate", v);
        return v;
    }

    private static void CtlLeaf(string f, bool v)
    {
        try
        {
            if (!AgObserved() || _ctlUnit == null) return;
            var u = _ctlUnit.UniqueId;
            Sched.Obs("ag_v", j => { j.Prop("f", f); j.Prop("u", u); j.Prop("v", v); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_v " + f, e); }
    }

    // ---- physics in Move ----

    private static string PhysCtx(out string u)
    {
        u = null;
        if (AgCtx.Count == 0) return "other";
        var (a, ctx) = AgCtx[AgCtx.Count - 1];
        u = UnitOfAgent(a);
        return ctx;
    }

    public static bool TapAgRaycast(Vector3 origin, Vector3 direction, out RaycastHit hitInfo, float maxDistance, int layerMask)
    {
        var hit = Physics.Raycast(origin, direction, out hitInfo, maxDistance, layerMask);
        try
        {
            if (AgObserved())
            {
                var ctx = PhysCtx(out var u);
                var d = hitInfo.distance;
                var pt = hitInfo.point;
                Sched.Obs("ag_phys", j =>
                {
                    j.Prop("fn", "ray"); j.Prop("ctx", ctx); j.Prop("u", u); j.Prop("o", origin); j.Prop("d", direction);
                    j.Prop("max", maxDistance); j.Prop("mask", layerMask); j.Prop("hit", hit);
                    if (hit) { j.Prop("dist", d); j.Prop("pt", pt); }
                });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe ag_phys ray", e); }
        return hit;
    }

    public static bool TapAgDiscCast(Vector3 origin, float radius, out RaycastHit hitInfo, float maxDistance, int layerMask)
    {
        var hit = PhysicUtils.HeuristicDiscCastDown(origin, radius, out hitInfo, maxDistance, layerMask);
        try
        {
            if (AgObserved())
            {
                var ctx = PhysCtx(out var u);
                var d = hitInfo.distance;
                var pt = hitInfo.point;
                Sched.Obs("ag_phys", j =>
                {
                    j.Prop("fn", "disc"); j.Prop("ctx", ctx); j.Prop("u", u); j.Prop("o", origin); j.Prop("r", radius);
                    j.Prop("max", maxDistance); j.Prop("mask", layerMask); j.Prop("hit", hit);
                    if (hit) { j.Prop("dist", d); j.Prop("pt", pt); }
                });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe ag_phys disc", e); }
        return hit;
    }

    // ---- requests and completions ----

    public static void AgPathToPrefix(UnitMovementAgent __instance, UnitCommand command, Vector3 destination, float approachRadius,
        float maxApproachRadius, bool distanceXZ, UnitMovementAgent destinationAgent)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var c = command == null ? null : Id(command);
            var da = UnitOfAgent(destinationAgent);
            var dself = ReferenceEquals(destinationAgent, __instance);
            var dhas = destinationAgent != null;
            var dview = DestViewId(destinationAgent, out var dattached);
            var dsame = ReferenceEquals(destinationAgent, AgDestAgent(__instance));
            var s = Snap(__instance);
            var tile = TileHandler.LastUpdateFrame;
            var frame = Time.frameCount;
            Sched.Obs("ag_req", j =>
            {
                j.Prop("fn", "pathto"); j.Prop("u", u); WriteId(j, "c", c); j.Prop("p", destination); j.Prop("r", approachRadius);
                j.Prop("mr", maxApproachRadius); j.Prop("xz", distanceXZ); j.Prop("da", da); j.Prop("dself", dself);
                j.Prop("dhas", dhas); j.Prop("dview", dview); j.Prop("dattached", dattached); j.Prop("dsame", dsame);
                j.Prop("tile", tile); j.Prop("frame", frame); WriteState(j, s);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_req pathto", e); }
    }

    public static void AgFindPathPrefix(UnitMovementAgent __instance, Vector3 destination, float? approachRadius, bool needLOS, float ignoreBlockerRadius)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var s = Snap(__instance);
            Sched.Obs("ag_req", j =>
            {
                j.Prop("fn", "find"); j.Prop("u", u); j.Prop("p", destination); NullableFloat(j, "r", approachRadius);
                j.Prop("los", needLOS); j.Prop("ibr", ignoreBlockerRadius); WriteState(j, s);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_req find", e); }
    }

    // The request either call left (RequestedPath for PathTo, the returned path for FindPath) and its parameters.
    public static void AgReqPostfix(UnitMovementAgent __instance, MethodBase __originalMethod) => AgReqEnd(__instance, __originalMethod.Name, null);

    public static void AgFindPathPostfix(UnitMovementAgent __instance, Path __result) => AgReqEnd(__instance, "FindPath", __result);

    private static void AgReqEnd(UnitMovementAgent a, string fn, Path ret)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(a);
            var s = Snap(a);
            var p = fn == "FindPath" ? ret : AgRequested(a);
            var info = PathInfo(p);
            Sched.Obs("ag_req_end", j => { j.Prop("u", u); j.Prop("fn", fn == "FindPath" ? "find" : "pathto"); info(j); WriteState(j, s); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_req_end", e); }
    }

    // A path's request parameters as the agent and the seeker set them (captured now, written later).
    private static Action<Json> PathInfo(Path p)
    {
        if (p == null) return j => j.Key("rq").W.WriteNull();
        var key = PathRef(p);
        var kind = p.GetType().Name;
        var tags = p.enabledTags;
        var mask = p.nnConstraint != null ? p.nnConstraint.graphMask.value : -1;
        var xz = p.nnConstraint != null && p.nnConstraint.distanceXZ;
        Vector3? start = null, end = null;
        if (p is ABPath ab) { start = ab.originalStartPoint; end = ab.originalEndPoint; }
        Vector3? clearance = null;
        float? approach = null, ignore = null, maxDist = null;
        bool? los = null;
        string ec = null;
        if (p is Kingmaker.TurnBasedMode.Pathfinding.ABPathWithClearance c) clearance = c.clearance;
        if (p is XPath x && x.endingCondition != null)
        {
            var cond = x.endingCondition;
            ec = cond.GetType().Name;
            var t = Traverse.Create(cond);
            if (t.Field("m_MaxDistance").FieldExists()) maxDist = t.Field("m_MaxDistance").GetValue<float>();
            if (t.Field("m_ApproachRadius").FieldExists()) approach = t.Field("m_ApproachRadius").GetValue<float>();
            if (t.Field("m_CheckLos").FieldExists()) los = t.Field("m_CheckLos").GetValue<bool>();
            if (t.Field("m_IgnoredBlockerRadius").FieldExists()) ignore = t.Field("m_IgnoredBlockerRadius").GetValue<float>();
        }
        return j =>
        {
            j.Obj("rq");
            j.Prop("path", key); j.Prop("kind", kind); j.Prop("tags", tags); j.Prop("gmask", mask); j.Prop("xz", xz);
            NullableVec(j, "start", start); NullableVec(j, "end", end); NullableVec(j, "clear", clearance);
            j.Prop("ec", ec); NullableFloat(j, "ar", approach); j.Prop("los", los); NullableFloat(j, "ibr", ignore);
            NullableFloat(j, "maxd", maxDist);
            j.W.WriteEndObject();
        };
    }

    public static void AgDonePrefix(UnitMovementAgent __instance, Path p, bool forcedPath)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var key = PathRef(p);
            var kind = p?.GetType().Name;
            var current = ReferenceEquals(p, AgRequested(__instance));
            var pts = p?.vectorPath?.ToList();
            var err = p != null && p.error;
            var state = p == null ? -1 : (int)p.CompleteState;
            var s = Snap(__instance);
            Sched.Obs("ag_done", j =>
            {
                j.Prop("u", u); j.Prop("path", key); j.Prop("kind", kind); j.Prop("forced", forcedPath); j.Prop("cur", current);
                j.Prop("err", err); j.Prop("cs", state);
                j.Arr("v"); if (pts != null) foreach (var x in pts) j.Vec(x); j.EndArr();
                WriteState(j, s);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_done", e); }
    }

    public static void AgDonePostfix(UnitMovementAgent __instance)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var s = Snap(__instance);
            Sched.Obs("ag_done_end", j => { j.Prop("u", u); WriteState(j, s); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_done_end", e); }
    }

    // ---- the agent's own calls ----

    public static void AgCallPrefix(UnitMovementAgent __instance, MethodBase __originalMethod, object[] __args)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var fn = __originalMethod.Name;
            var origin = Origin();
            int? index = fn == "SetWaypoint" && __args.Length > 0 && __args[0] is int i ? i : (int?)null;
            Action<Json> path = null;
            if ((fn == "ForcePath" || fn == "FollowPrecomputedPath") && __args.Length > 0 && __args[0] is Path p)
            {
                var info = PathInfo(p);
                var pts = p.vectorPath?.ToList();
                var rest = __args.Skip(1).Select(x => x is bool b ? (b ? "1" : "0") : x is float f ? F(f) : "?").ToList();
                path = j =>
                {
                    info(j);
                    j.Arr("v"); if (pts != null) foreach (var x in pts) j.Vec(x); j.EndArr();
                    j.Arr("args"); foreach (var a in rest) j.W.WriteValue(a); j.EndArr();
                };
            }
            Sched.Obs("ag_call", j =>
            {
                j.Prop("fn", fn); j.Prop("u", u); j.Prop("origin", origin); j.Prop("i", index);
                path?.Invoke(j);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_call", e); }
    }

    public static void AgCallPostfix(UnitMovementAgent __instance, MethodBase __originalMethod)
    {
        try
        {
            if (!AgObserved()) return;
            var u = UnitOfAgent(__instance);
            var fn = __originalMethod.Name;
            var s = Snap(__instance);
            Sched.Obs("ag_call_end", j => { j.Prop("fn", fn); j.Prop("u", u); WriteState(j, s); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_call_end", e); }
    }

    // ---- UnitMoveController.TickUnit ----

    public static void AgCtlPrefix(UnitEntityData unit)
    {
        _ctlUnit = unit;
        try
        {
            if (!AgObserved() || unit == null) return;
            var u = unit.UniqueId;
            var pos = unit.Position;
            var prev = unit.PreviousPosition;
            Sched.Obs("ag_ctl", j => { j.Prop("u", u); j.Prop("pos", pos); j.Prop("prev", prev); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_ctl", e); }
    }

    public static void AgCtlFinalizer(UnitEntityData unit)
    {
        _ctlUnit = null;
        try
        {
            if (!AgObserved() || unit == null) return;
            var u = unit.UniqueId;
            var pos = unit.Position;
            var orient = unit.Orientation;
            var view = unit.View != null ? unit.View.transform.position : (Vector3?)null;
            Sched.Obs("ag_ctl_end", j => { j.Prop("u", u); j.Prop("pos", pos); j.Prop("orient", orient); NullableVec(j, "vpos", view); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_ctl_end", e); }
    }

    // ---- ObstaclesHelper.ConnectToGroups: its first `unit.Unit.EntityData.Group` (the getter resolves the group lazily,
    // UpdateGroup) decides the early return; the tap records that evaluation's Memory != null ----

    private static UnitMovementAgentBase _cgAgent;
    private static bool _cgFirst;

    public static void AgConnectPrefix(UnitMovementAgentBase unit)
    {
        _cgAgent = unit;
        _cgFirst = true;
    }

    public static void AgConnectFinalizer()
    {
        _cgAgent = null;
        _cgFirst = false;
    }

    public static Kingmaker.UnitLogic.Groups.UnitGroup TapAgGroup(UnitEntityData data)
    {
        var g = data.Group;
        try
        {
            if (_cgFirst && AgObserved())
            {
                _cgFirst = false;
                var u = UnitOf(_cgAgent);
                var mem = g?.Memory != null;
                Sched.Obs("ag_v", j => { j.Prop("f", "gmem"); j.Prop("u", u); j.Prop("v", mem); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe ag_v gmem", e); }
        return g;
    }

    // ---- the agent's calls into the unit (UnitEntityView.OnMovementInterrupted, OnMovementComplete,
    // OnMovementWaypointUpdate) ----

    public static void AgCbPrefix(UnitEntityView __instance, MethodBase __originalMethod, object[] __args)
    {
        try
        {
            if (!AgObserved()) return;
            var u = __instance != null ? __instance.EntityData?.UniqueId : null;
            var fn = __originalMethod.Name;
            Vector3? dest = __args.Length > 0 && __args[0] is Vector3 v ? v : (Vector3?)null;
            int? index = __args.Length > 0 && __args[0] is int i ? i : (int?)null;
            Sched.Obs("ag_cb", j => { j.Prop("fn", fn); j.Prop("u", u); NullableVec(j, "dest", dest); j.Prop("i", index); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_cb", e); }
    }

    public static void AgCbPostfix(UnitEntityView __instance, MethodBase __originalMethod)
    {
        try
        {
            if (!AgObserved()) return;
            var u = __instance != null ? __instance.EntityData?.UniqueId : null;
            var fn = __originalMethod.Name;
            Sched.Obs("ag_cb_end", j => { j.Prop("fn", fn); j.Prop("u", u); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_cb_end", e); }
    }

    // ---- UnitEntityData.UpdateSlowRotation(maxAngle): its inputs ----

    private static readonly AccessTools.FieldRef<UnitEntityData, float> OrientationField =
        AccessTools.FieldRefAccess<UnitEntityData, float>("m_Orientation");

    public static void AgRotPrefix(UnitEntityData __instance, float maxAngle)
    {
        try
        {
            if (!AgObserved() || __instance == null) return;
            var u = __instance.UniqueId;
            var o = OrientationField(__instance);
            var desired = __instance.DesiredOrientation;
            var ctl = ReferenceEquals(_ctlUnit, __instance);
            Sched.Obs("ag_rot", j => { j.Prop("u", u); j.Prop("ctl", ctl); j.Prop("o", o); j.Prop("desired", desired); j.Prop("max", maxAngle); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_rot", e); }
    }

    // ---- UnitPlaceOnGroundController.TickUnit (physics; an input for now) ----

    public static void AgGroundPrefix(UnitEntityData data, out Vector3? __state)
    {
        __state = null;
        try { if (AgObserved() && data != null) __state = data.Position; }
        catch (Exception e) { Trace.HookFailed("observe ag_ground", e); }
    }

    public static void AgGroundFinalizer(UnitEntityData data, Vector3? __state)
    {
        try
        {
            if (!AgObserved() || data == null || !__state.HasValue) return;
            var u = data.UniqueId;
            var before = __state.Value;
            var after = data.Position;
            var inCtl = ReferenceEquals(_ctlUnit, data);
            Sched.Obs("ag_ground", j => { j.Prop("u", u); j.Prop("ctl", inCtl); j.Prop("pos", before); j.Prop("after", after); });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_ground", e); }
    }

    // ---- taps ----

    // The taps by the call they replace: (method, declaring type, name, parameter types, tap, expected count).
    private static List<(MethodBase m, Type type, string name, Type[] args, string tap, int want)> AgTapSites()
    {
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        MethodBase M(Type t, string n) => t.GetMethods(all).Single(m => m.Name == n);
        var move = AgBase.GetMethods(all).Single(m => m.Name == "Move" && m.GetParameters().Length == 5);
        var moveUnit = M(AgBase, "MoveUnit");
        var tick = M(Ag, "TickMovement");
        var can = M(Ag, "CanMoveInTurnBased");
        var slow = M(Ag, "SlowDown");
        var ctl = M(typeof(UnitMoveController), "TickUnit");
        var pathTo = M(Ag, "PathTo");
        var find = M(Ag, "FindPath");
        var really = AccessTools.PropertyGetter(Ag, nameof(UnitMovementAgent.IsReallyMoving));
        var failed = AccessTools.PropertyGetter(Ag, nameof(UnitMovementAgent.PathFailed));
        var approach = M(Ag, "CheckApproachRadius");
        var slowRot = M(typeof(UnitEntityData), nameof(UnitEntityData.UpdateSlowRotation));
        var connect = M(typeof(ObstaclesHelper), nameof(ObstaclesHelper.ConnectToGroups));
        var raycastArgs = new[] { typeof(Vector3), typeof(Vector3), typeof(RaycastHit).MakeByRefType(), typeof(float), typeof(int) };
        var discArgs = new[] { typeof(Vector3), typeof(float), typeof(RaycastHit).MakeByRefType(), typeof(float), typeof(int) };
        return new List<(MethodBase, Type, string, Type[], string, int)>
        {
            (move, typeof(Physics), nameof(Physics.Raycast), raycastArgs, nameof(TapAgRaycast), 1),
            (move, typeof(PhysicUtils), nameof(PhysicUtils.HeuristicDiscCastDown), discArgs, nameof(TapAgDiscCast), 1),
            (moveUnit, typeof(UnitEntityData), "get_FlyHeight", Type.EmptyTypes, nameof(TapAgFlyHeight), 1),
            (tick, typeof(UnitAnimationManager), "get_IsPreventingMovement", Type.EmptyTypes, nameof(TapAgAnimPrevents), 1),
            (tick, typeof(UnitEntityView), "get_IsCommandsPreventMovement", Type.EmptyTypes, nameof(TapAgCommandsPrevent), 1),
            (tick, AgBase, "get_MaxSpeed", Type.EmptyTypes, nameof(TapAgMaxSpeed), 2),
            (can, typeof(UnitAnimationManager), "get_IsPreventingMovement", Type.EmptyTypes, nameof(TapAgAnimPrevents), 1),
            (can, typeof(UnitEntityView), "get_IsCommandsPreventMovement", Type.EmptyTypes, nameof(TapAgCommandsPrevent), 1),
            (can, typeof(UnitEntityData), "get_CutsceneControlledUnit", Type.EmptyTypes, nameof(TapAgCutsceneControl), 1),
            (slow, typeof(Kingmaker.Game), "get_IsControllerGamepad", Type.EmptyTypes, nameof(TapAgGamepad), 1),
            (ctl, typeof(UnitState), "get_CanMove", Type.EmptyTypes, nameof(TapCtlCanMove), 2),
            (ctl, typeof(UnitState), "get_CanRotate", Type.EmptyTypes, nameof(TapCtlCanRotate), 1),
            (slowRot, typeof(UnitState), "get_CanRotate", Type.EmptyTypes, nameof(TapCtlCanRotate), 1),
            (connect, typeof(UnitEntityData), "get_Group", Type.EmptyTypes, nameof(TapAgGroup), 4),
            (pathTo, typeof(Path), nameof(Path.IsDone), Type.EmptyTypes, nameof(TapAgIsDone), 1),
            (find, typeof(Path), nameof(Path.IsDone), Type.EmptyTypes, nameof(TapAgIsDone), 2),
            (really, typeof(Path), nameof(Path.IsDone), Type.EmptyTypes, nameof(TapAgIsDone), 1),
            (failed, typeof(Path), nameof(Path.IsDone), Type.EmptyTypes, nameof(TapAgIsDone), 1),
            (can, typeof(Kingmaker.AreaLogic.Cutscenes.CutsceneControlledUnit), nameof(Kingmaker.AreaLogic.Cutscenes.CutsceneControlledUnit.GetCurrentlyActive), Type.EmptyTypes, nameof(TapAgCutsceneActive), 1),
            (approach, typeof(UnitEntityView), "get_SoftCollider", Type.EmptyTypes, nameof(TapAgSoftCollider), 1),
            (approach, typeof(Collider), nameof(Collider.ClosestPoint), new[] { typeof(Vector3) }, nameof(TapAgClosestPoint), 1),
            (tick, typeof(Kingmaker.EntitySystem.EntityService.Proxy), "get_Entity", Type.EmptyTypes, nameof(TapAgProxyEntity), 1),
        };
    }

    private static List<(MethodBase m, Type type, string name, Type[] args, string tap, int want)> _agTaps;

    public static IEnumerable<CodeInstruction> AgTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase __originalMethod)
    {
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodInfo mb)
            {
                foreach (var t in _agTaps)
                {
                    if (t.m.MethodHandle != __originalMethod.MethodHandle || mb.DeclaringType != t.type || mb.Name != t.name) continue;
                    if (!mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(t.args)) continue;
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), t.tap);
                    var k = "agent:" + __originalMethod.Name + ":" + t.tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            yield return i;
        }
    }

    // ---- install ----

    private static void InstallAgent(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        void Fail(string what) => Health.Failed("agent observe: " + what, new Exception("agent"));
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        MethodBase M(Type t, string n, int? arity = null)
        {
            var ms = t.GetMethods(all).Where(m => m.Name == n && (arity == null || m.GetParameters().Length == arity)).ToList();
            if (ms.Count != 1) { Fail($"{t.Name}.{n} matched {ms.Count} methods"); return null; }
            return ms[0];
        }
        try { _agTaps = AgTapSites(); }
        catch (Exception e) { Health.Failed("agent observe: tap sites", e); return; }
        var failed = Health.FailedCount;
        foreach (var m in _agTaps.Select(t => t.m).Distinct())
        {
            try { h.Patch(m, transpiler: new HarmonyMethod(typeof(CommandObs), nameof(AgTapTranspiler))); }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name + " (agent taps)", e); }
        }
        foreach (var t in _agTaps)
        {
            var k = "agent:" + t.m.Name + ":" + t.tap;
            var want = _agTaps.Where(x => x.m == t.m && x.tap == t.tap).Sum(x => x.want);
            Tapped.TryGetValue(k, out var n);
            if (n != want) Fail("tap " + k + " replaced " + n + " calls, not " + want);
        }

        patch(M(Ag, "TickMovement"), nameof(AgTickPrefix), nameof(AgTickPostfix), nameof(AgCtxFinalizer));
        patch(M(Ag, "SimulateTickMovementForTurnBased"), nameof(AgSimPrefix), null, nameof(AgCtxFinalizer));
        patch(M(Ag, "CanMoveInTurnBased"), null, nameof(AgCanMovePostfix), null);
        patch(M(Ag, "PathTo"), nameof(AgPathToPrefix), nameof(AgReqPostfix), null);
        patch(M(Ag, "FindPath"), nameof(AgFindPathPrefix), nameof(AgFindPathPostfix), null);
        patch(M(Ag, "OnPathComplete", 2), nameof(AgDonePrefix), nameof(AgDonePostfix), null);
        foreach (var n in new[] { "Stop", "CompleteMovement", "InterruptMovement", "SetWaypoint", "ForcePath", "FollowPrecomputedPath" })
            patch(M(Ag, n), nameof(AgCallPrefix), nameof(AgCallPostfix), null);
        patch(M(typeof(UnitMoveController), "TickUnit"), nameof(AgCtlPrefix), null, nameof(AgCtlFinalizer));
        patch(M(typeof(UnitPlaceOnGroundController), "TickUnit"), nameof(AgGroundPrefix), null, nameof(AgGroundFinalizer));
        patch(M(typeof(UnitEntityData), nameof(UnitEntityData.UpdateSlowRotation)), nameof(AgRotPrefix), null, null);
        patch(M(typeof(ObstaclesHelper), nameof(ObstaclesHelper.ConnectToGroups)), nameof(AgConnectPrefix), null, nameof(AgConnectFinalizer));
        foreach (var n in new[] { "OnMovementInterrupted", "OnMovementComplete", "OnMovementWaypointUpdate" })
            patch(M(typeof(UnitEntityView), n), nameof(AgCbPrefix), nameof(AgCbPostfix), null);
        AgOn = Health.FailedCount == failed;
    }

    internal static void ResetAgentWindow()
    {
        AgCtx.Clear();
        _ctlUnit = null;
        _pathIds = new ConditionalWeakTable<Path, PathBox>();
        _nextPath = 0;
        PathDefined.Clear();
        PathPending.Clear();
    }
}
