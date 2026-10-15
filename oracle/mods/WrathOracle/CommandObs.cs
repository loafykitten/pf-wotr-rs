using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Runtime.CompilerServices;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Units;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;
using Kingmaker.Items;
using Kingmaker.UnitLogic;
using Kingmaker.Utility;
using Kingmaker.Visual.Animation.Actions;
using Kingmaker.View;
using TurnBased.Controllers;

namespace WrathOracle;

// Observations for the units' commands (T1 "obs" records, inside a turn-based window), so a replay runs
// the commands' base lifecycle (UnitCommands, UnitCommandController, UnitCommand's base methods, the
// turn's running commands) and takes as inputs only what that code reads or calls outside itself
// (docs/poc5/commands.md, slice A):
//  - uc_new: a command's description at first sight (class and base classes, type, target, flags, and
//    which of the lifecycle's virtual methods its class overrides);
//  - uc_state / uc_cs / uc_running: every command set's slots, queue and parked command, every
//    command's state, the current turn's running commands, as changes, at the window's first tick
//    (then uc_snap) and around UnitCommandController's tick (then uc_ctl, uc_ctl_end);
//  - entries, where the lifecycle code starts, with their callers: uc_run (UnitCommands.Run, the
//    private overload) and uc_run_end, uc_queue (AddToQueueOrRun), uc_tick (TickOnUnit, for a unit
//    with a command, a queue or a parked command), uc_x (InterruptAll, InterruptAiCommands,
//    InterruptMove, InterruptGroupCommand, RemoveNoStartStadartCommand, RemoveFinishedAndUpdateQueue
//    from outside the controller) and uc_x_end, uc_set (ReplaceCommands: the predictions' sets);
//  - uc_tc (TickCommand): its canMoveUnit argument;
//  - uc_v: a leaf read by the base lifecycle, with its result (positions and line of sight, the view
//    and the movement agent, the hands, the class's answers, ...). A read is a leaf only when every
//    frame between it and the innermost entry is base lifecycle code (a stack walk): the same getter
//    called from class code, an event handler or TickApproaching isn't one;
//  - uc_fx: class code the lifecycle calls (OnStart, OnTick, OnAction, TickApproaching, Init, an
//    overridden OnRun), with the base state it leaves (and OnAction's result);
//  - uc_end (OnEnded): an end, with whether base lifecycle code made it ("base") or not ("other");
//  - uc_rc (TurnController.AddRunningCommand / RemoveRunningCommand): every call;
//  - uc_exc: an exception out of TickCommand (TickOnUnit catches it).
// Observer "attacks" (slice B): UnitAttack's code is base lifecycle code too (its leaves are recorded: five,
// efa, farm, and the reads a transpiler taps, afin, arel, style, enemies, sorted), ua_atk records the attack list
// InitAttacks leaves (what ConfigureAnimations decides is an input), and a UnitAttack's uc_fx carries its class
// state (ua).
// Observer "approach" (slice C): TickApproaching is base lifecycle code (no longer uc_fx), as is the code of UnitMoveTo
// and UnitAttackOfOpportunity (their uc_fx are checks); the reads a transpiler taps in them are leaves (spart, rpart,
// tact, wants, hasmove, canmove, ucgu, fpath, pvis, vpath, tview; pmf; threat, anim, idle); uc_call records the lifecycle's
// calls into the movement agent (MoveTo, FollowPrecomputedPath, StopMoving); uc_new describes the approach (ap) and
// a UnitMoveTo's (mv) and an attack of opportunity's (aoo) own fields.
// Numbers in rows are written in the invariant culture (the game sets the thread's culture to its
// language's). Everything is read through fields and auto-properties (a target by its reference's id, never
// TargetWrapper.Unit, which resolves it); the leaves' values are the game's own evaluations
// (postfixes). Every hook body catches its own exceptions.
internal static partial class CommandObs
{
    private sealed class Box { public int V; }

    private static ConditionalWeakTable<UnitCommand, Box> _cmdIds = new ConditionalWeakTable<UnitCommand, Box>();
    private static ConditionalWeakTable<UnitCommands, Box> _setIds = new ConditionalWeakTable<UnitCommands, Box>();
    private static ConditionalWeakTable<TargetWrapper, Box> _targetIds = new ConditionalWeakTable<TargetWrapper, Box>();
    private static int _nextCmd, _nextSet, _nextTarget;
    private static bool _opened;
    private static readonly Dictionary<int, string> LastSet = new Dictionary<int, string>();
    private static readonly Dictionary<int, string> LastCmd = new Dictionary<int, string>();
    private static string _lastRunning;

    private static bool On => Sched.On;

    public static void ResetWindow()
    {
        _cmdIds = new ConditionalWeakTable<UnitCommand, Box>();
        _setIds = new ConditionalWeakTable<UnitCommands, Box>();
        _targetIds = new ConditionalWeakTable<TargetWrapper, Box>();
        _nextCmd = _nextSet = _nextTarget = 0;
        _opened = false;
        LastSet.Clear();
        LastCmd.Clear();
        _lastRunning = null;
        Entries.Clear();
        _fx.Clear();
        _fxCmd.Clear();
        _atk.Clear();
        _atkCfg.Clear();
        Uct.Clear();
        ResetAgentWindow();
    }

    // ---- fields ----

    private static readonly AccessTools.FieldRef<UnitCommands, UnitEntityData> Owner =
        AccessTools.FieldRefAccess<UnitCommands, UnitEntityData>("m_Owner");
    private static readonly AccessTools.FieldRef<UnitCommand, bool> ActHandled =
        AccessTools.FieldRefAccess<UnitCommand, bool>("m_IsAnimationActHandled");
    private static readonly AccessTools.FieldRef<UnitCommand, bool> HasAnimationField =
        AccessTools.FieldRefAccess<UnitCommand, bool>("HasAnimation");
    private static readonly AccessTools.FieldRef<UnitCommand, bool> IgnoreCooldownField =
        AccessTools.FieldRefAccess<UnitCommand, bool>("m_IgnoreCooldown");
    private static readonly AccessTools.FieldRef<UnitCommand, TimeSpan?> IgnoreBefore =
        AccessTools.FieldRefAccess<UnitCommand, TimeSpan?>("m_IgnoreCooldownBeforeTime");
    private static readonly PropertyInfo PretendActTime = AccessTools.Property(typeof(UnitCommand), "PretendActTime");
    private static readonly PropertyInfo DidRun = AccessTools.Property(typeof(UnitCommand), "DidRun");
    private static readonly AccessTools.FieldRef<TargetWrapper, UnityEngine.Vector3> TargetPoint =
        AccessTools.FieldRefAccess<TargetWrapper, UnityEngine.Vector3>("m_Point");
    private static readonly AccessTools.FieldRef<TurnController, List<UnitCommand>> Running =
        AccessTools.FieldRefAccess<TurnController, List<UnitCommand>>("m_RunningCommands");
    private static readonly AccessTools.FieldRef<TurnController, UnitCommands> PredictionRider =
        AccessTools.FieldRefAccess<TurnController, UnitCommands>("m_PredictionCommandsRider");
    private static readonly AccessTools.FieldRef<TurnController, UnitCommands> PredictionMount =
        AccessTools.FieldRefAccess<TurnController, UnitCommands>("m_PredictionCommandsMount");

    // ---- identity ----

    // A command's id; at first sight its description (uc_new) is written first. Call outside a record's
    // body.
    internal static int? Id(UnitCommand c)
    {
        if (c == null) return null;
        if (_cmdIds.TryGetValue(c, out var b)) return b.V;
        b = new Box { V = ++_nextCmd };
        _cmdIds.Add(c, b);
        var id = b.V;
        Sched.Obs("uc_new", j => { j.Prop("c", id); Describe(j, c); });
        return id;
    }

    private static int? SetId(UnitCommands s)
    {
        if (s == null) return null;
        if (_setIds.TryGetValue(s, out var b)) return b.V;
        b = new Box { V = ++_nextSet };
        _setIds.Add(s, b);
        return b.V;
    }

    // A target as rows have it: empty (null), the unit's reference id or "@x,y,z" (round-trip), then
    // "#" and the wrapper's id (object.Equals between two commands' targets is true for one wrapper
    // without calling TargetWrapper.Equals).
    private static string TargetStr(TargetWrapper t)
    {
        if (t == null) return "";
        if (!_targetIds.TryGetValue(t, out var b))
        {
            b = new Box { V = ++_nextTarget };
            _targetIds.Add(t, b);
        }
        var p = TargetPoint(t);
        var what = !string.IsNullOrEmpty(t.UnitRef.Id) ? t.UnitRef.Id : "@" + p.x.ToString("R", CultureInfo.InvariantCulture) + "," + p.y.ToString("R", CultureInfo.InvariantCulture) + "," + p.z.ToString("R", CultureInfo.InvariantCulture);
        return what + "#" + b.V;
    }

    private static void WriteId(Json j, string k, int? v)
    {
        j.Key(k);
        if (v.HasValue) j.W.WriteValue(v.Value); else j.W.WriteNull();
    }

    // The lifecycle's virtual members, by name: which of them a command's class overrides.
    private static readonly string[] Virtuals =
    {
        "Init", "OnRun", "OnStart", "OnTick", "OnAction", "OnEnded", "TickApproaching", "TurnToTarget", "TryMergeInto",
        "get_ShouldBeInterrupted", "get_IsInterruptible", "get_CanStart", "get_AwaitMovementFinish",
        "get_DontWaitForHands", "get_IsOneFrameCommand", "get_CanMoveAfterStart", "get_ShouldTurnToTarget",
        "get_NeedEquipWeapons", "ConvertToOneFrame", "AddRiderCommand", "OnTickApproaching",
    };

    private static readonly Dictionary<Type, string> Overrides = new Dictionary<Type, string>();

    private static string OverridesOf(Type t)
    {
        if (Overrides.TryGetValue(t, out var s)) return s;
        var names = new List<string>();
        foreach (var n in Virtuals)
        {
            for (var x = t; x != null && x != typeof(UnitCommand); x = x.BaseType)
            {
                if (x.GetMethods(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly).Any(m => m.Name == n))
                {
                    names.Add(n);
                    break;
                }
            }
        }
        s = string.Join(",", names);
        Overrides[t] = s;
        return s;
    }

    private static void Describe(Json j, UnitCommand c)
    {
        j.Prop("class", c.GetType().Name);
        j.Prop("type", (int)c.Type);
        var t = c.Target;
        j.Prop("tgt", t?.UnitRef.Id);
        if (t != null && string.IsNullOrEmpty(t.UnitRef.Id)) j.Prop("pt", TargetPoint(t)); else { j.Key("pt"); j.W.WriteNull(); }
        j.Prop("exec", c.Executor?.UniqueId);
        j.Prop("ai", c.AiAction != null);
        j.Prop("react", c.ReactionAction);
        j.Prop("ign", IgnoreCooldownField(c));
        var before = IgnoreBefore(c);
        j.Key("ign_before"); if (before.HasValue) j.W.WriteValue(before.Value.Ticks); else j.W.WriteNull();
        j.Prop("cut", c.Cutscene);
        j.Prop("dit", c.DontInterruptInTurnBasedMode);
        j.Prop("player", c.CreatedByPlayer);
        j.Prop("rider", c.RiderCommand != null);
        j.Prop("mount", c.MountCommand != null);
        j.Prop("ovr", OverridesOf(c.GetType()));
        // Its base classes below UnitCommand (the lifecycle's type tests: `is UnitGroupCommand`).
        var bases = new List<string>();
        for (var b = c.GetType().BaseType; b != null && b != typeof(UnitCommand); b = b.BaseType) bases.Add(b.Name);
        j.Prop("bases", string.Join(",", bases));
        // A UnitAttack's flags (set by its creator before Run): IsSingleAttack, ForceFullAttack, IsCharge,
        // OnlyForInspect, an OverrideAction.
        if (AttacksOn && c is UnitAttack a) j.Prop("ua", AttackFlags(a));
        // The approach's own fields (observer "approach"), as the creator left them.
        if (ApproachOn)
        {
            j.Prop("ap", ApproachStr(c));
            if (c is UnitMoveTo mt) j.Prop("mv", MoveToStr(mt));
            if (c is UnitAttackOfOpportunity ao) j.Prop("aoo", B(ao.ForceFlatFooted) + B(AooTurn(ao)));
        }
        j.Prop("s", Row(c));
    }

    private static string B(bool b) => b ? "1" : "0";

    // "started acted finished|result|TimeSinceStart|actHandled HasAnimation animation animActed animFinished|
    //  PretendActTime|HandAnimationWaitTime|NextApproachTime|FinishedApproaching IsMoveUnit DidRun|
    //  InterruptAsSoonAsPossible ShouldSkipAnimation|type|target|executor" (flags 0/1, '-' without an animation).
    internal static string Row(UnitCommand c)
    {
        var a = c.Animation;
        var tgt = TargetStr(c.Target);
        return B(c.IsStarted) + B(c.IsActed) + B(c.IsFinished) + "|" + (int)c.Result + "|" + c.TimeSinceStart.ToString("R", CultureInfo.InvariantCulture) + "|" +
               B(ActHandled(c)) + B(HasAnimationField(c)) + B(a != null) + (a == null ? "-" : B(a.IsActed)) + (a == null ? "-" : B(a.IsFinished)) + "|" +
               ((float)PretendActTime.GetValue(c)).ToString("R", CultureInfo.InvariantCulture) + "|" + c.HandAnimationWaitTime.ToString("R", CultureInfo.InvariantCulture) + "|" + c.NextApproachTime.Ticks + "|" +
               B(c.FinishedApproaching) + B(c.IsMoveUnit) + B((bool)DidRun.GetValue(c)) + "|" + B(c.InterruptAsSoonAsPossible) + B(c.ShouldSkipAnimation) + "|" +
               (int)c.Type + "|" + tgt + "|" + (c.Executor?.UniqueId ?? "");
    }

    private static string SetRow(UnitCommands s)
    {
        string Ids(IEnumerable<UnitCommand> cs) => string.Join(",", cs.Select(x => x == null ? "-" : Id(x).ToString()));
        return (Owner(s)?.UniqueId ?? "") + "|" + B(s.IsSimulating) + "|" + Ids(s.Raw) + "|" + Ids(s.Queue) + "|" +
               (s.PreviousCommand == null ? "-" : Id(s.PreviousCommand).ToString()) + "|" + B(s.GroupCommand != null);
    }

    // ---- digests ----

    // Every set a unit holds (and the current turn's prediction sets), every command in them, and the
    // current turn's running commands, as changes.
    private static void Digest()
    {
        var game = Game.Instance;
        var sets = new List<(UnitCommands set, string unit)>();
        foreach (var u in StateDump.AllUnits(game))
            if (u != null && !u.IsDisposed && u.Commands != null) sets.Add((u.Commands, u.UniqueId));
        var turn = game.TurnBasedCombatController?.CurrentTurn;
        if (turn != null)
        {
            if (PredictionRider(turn) != null) sets.Add((PredictionRider(turn), null));
            if (PredictionMount(turn) != null) sets.Add((PredictionMount(turn), null));
        }
        var cmds = new List<UnitCommand>();
        foreach (var (s, unit) in sets)
        {
            var sid = SetId(s).Value;
            var row = SetRow(s);
            if (!LastSet.TryGetValue(sid, out var old) || old != row)
            {
                LastSet[sid] = row;
                Sched.Obs("uc_state", j => { j.Prop("set", sid); j.Prop("u", unit); j.Prop("s", row); });
            }
            cmds.AddRange(s.Raw.Where(c => c != null));
            cmds.AddRange(s.Queue.Where(c => c != null));
            if (s.PreviousCommand != null) cmds.Add(s.PreviousCommand);
        }
        var running = turn == null ? new List<UnitCommand>() : Running(turn);
        cmds.AddRange(running.Where(c => c != null));
        foreach (var c in cmds.Distinct())
        {
            var id = Id(c).Value;
            var row = Row(c);
            if (LastCmd.TryGetValue(id, out var old) && old == row) continue;
            LastCmd[id] = row;
            Sched.Obs("uc_cs", j => { j.Prop("c", id); j.Prop("s", row); });
        }
        var runRow = turn == null ? "-" : string.Join(",", running.Select(c => c == null ? "null" : Id(c).ToString()));
        if (runRow != _lastRunning)
        {
            _lastRunning = runRow;
            Sched.Obs("uc_running", j => j.Prop("s", runRow));
        }
    }

    // Called from Observe.Tick (GameMode.Tick's prefix) inside a turn-based window: the first tick's state.
    public static void Tick()
    {
        if (_opened || !On) return;
        _opened = true;
        try
        {
            Digest();
            Sched.Obs("uc_snap");
            if (EngOn) EngInit();
            if (TgtOn) TgtInit();
            if (ImpOn) ImpInit();
            if (AgOn) AgInit();
        }
        catch (Exception e) { Trace.HookFailed("observe uc_snap", e); }
    }

    // BaseUnitController.Tick, for UnitCommandController.
    public static void ControllerTickPrefix(BaseUnitController __instance)
    {
        if (!On || !(__instance is UnitCommandController)) return;
        try { Digest(); Sched.Obs("uc_ctl"); }
        catch (Exception e) { Trace.HookFailed("observe uc_ctl", e); }
    }

    public static void ControllerTickFinalizer(BaseUnitController __instance, Exception __exception)
    {
        if (!On || !(__instance is UnitCommandController)) return;
        try
        {
            Digest();
            var exc = __exception != null;
            Sched.Obs("uc_ctl_end", j => j.Prop("exc", exc));
        }
        catch (Exception e) { Trace.HookFailed("observe uc_ctl_end", e); }
    }

    // ---- the stack walk ----

    // Entries (where lifecycle code starts) and base lifecycle methods, by method handle.
    private static readonly HashSet<IntPtr> EntrySet = new HashSet<IntPtr>();
    private static readonly HashSet<IntPtr> BaseSet = new HashSet<IntPtr>();
    // Frames that are neither (System.Object.Equals between TryAddToQueueInsteadOfRunImmediately and
    // TargetWrapper.Equals).
    private static readonly HashSet<IntPtr> Transparent = new HashSet<IntPtr>();
    // Generic methods (by their definition) that are neither: LinqExtensions.ForEach between the engagement
    // controller's Tick and its lambdas.
    private static readonly HashSet<IntPtr> TransparentGeneric = new HashSet<IntPtr>();

    // The entries on the stack (the innermost last), and whether each one's lifecycle is recorded.
    private struct Entry { public bool Rec, Ext, Ai, Eng, Imp; }
    private static readonly List<Entry> Entries = new List<Entry>();

    private static bool InRecordedEntry => Entries.Count > 0 && Entries[Entries.Count - 1].Rec;

    private static bool Skip(MethodBase m)
    {
        if (m == null || m.DeclaringType == null) return true;
        if (m.Name.StartsWith("DMD<", StringComparison.Ordinal)) return true;
        var ns = m.DeclaringType.Namespace ?? "";
        return ns.StartsWith("WrathOracle", StringComparison.Ordinal) || ns.StartsWith("HarmonyLib", StringComparison.Ordinal) ||
               ns.StartsWith("MonoMod", StringComparison.Ordinal);
    }

    // 1: every frame between `self` (the hooked method, whose own frames are skipped) and the innermost
    // entry is base lifecycle code; 0: some frame isn't; -1: no entry on the stack. With `sys`, frames of
    // System.Linq and System.Collections.Generic (and System.Array) are passed (UnitAttack.UpdateTarget's
    // Where, ToList and Sort).
    private static int Chain(MethodBase self, bool sys = false)
    {
        var frames = new StackTrace(1, false).GetFrames();
        if (frames == null) return -1;
        var h = self?.MethodHandle.Value ?? IntPtr.Zero;
        var started = false;
        foreach (var f in frames)
        {
            var m = Rng.FrameMethod(f);
            if (Skip(m)) continue;
            var mh = m.MethodHandle.Value;
            if (!started)
            {
                if (mh == h) continue;
                started = true;
            }
            if (EntrySet.Contains(mh)) return 1;
            if (BaseSet.Contains(mh) || Transparent.Contains(mh)) continue;
            if (m is MethodInfo mi && mi.IsGenericMethod && TransparentGeneric.Contains(mi.GetGenericMethodDefinition().MethodHandle.Value)) continue;
            if (sys && IsSys(m)) continue;
            return 0;
        }
        return -1;
    }

    private static bool IsSys(MethodBase m)
    {
        var t = m.DeclaringType;
        var ns = t.Namespace ?? "";
        return ns == "System.Linq" || ns == "System.Collections.Generic" || (ns == "System" && t == typeof(Array));
    }

    // The game's methods calling the hooked one, innermost first (the hooked method's own frame first),
    // joined by " < " (as Rules' origin).
    private static string Origin()
    {
        try
        {
            var frames = new StackTrace(1, false).GetFrames();
            if (frames == null) return "?";
            var ms = frames.Select(Rng.FrameMethod).Where(m => !Skip(m)).ToList();
            // Deep enough to reach the frame's root (GameMode.Tick, a coroutine, the UI's LateUpdate);
            // a longer stack ends in "...".
            var s = string.Join(" < ", ms.Take(64).Select(m => m.DeclaringType.FullName + "." + m.Name));
            return ms.Count > 64 ? s + " < ..." : s;
        }
        catch (Exception) { return "?"; }
    }

    // ---- entries ----

    private static void Push(bool rec, bool ext, bool ai = false, bool eng = false, bool imp = false) => Entries.Add(new Entry { Rec = rec, Ext = ext, Ai = ai, Eng = eng, Imp = imp });

    // The innermost entry, removed: whether its start record was written.
    private static bool Pop()
    {
        if (Entries.Count == 0) return false;
        var e = Entries[Entries.Count - 1];
        Entries.RemoveAt(Entries.Count - 1);
        return e.Ext;
    }

    // An entry starts: nested when base lifecycle code of an outer entry calls it (then it is that
    // entry's code, recorded as that one is), else its own. Returns whether it is recorded, and pushes it.
    private static bool Enter(MethodBase m, out bool nested, bool ai = false, bool eng = false, bool imp = false)
    {
        nested = false;
        if (!On) { Push(false, false, ai, eng, imp); return false; }
        bool rec;
        try
        {
            nested = Entries.Count > 0 && Chain(m) == 1;
            rec = !nested || InRecordedEntry;
        }
        catch (Exception e) { Trace.HookFailed("observe entry", e); rec = false; }
        Push(rec, rec, ai, eng, imp);
        return rec;
    }

    // UnitCommands.Run(cmd, fromQueue, doNotClearQueue).
    public static void RunPrefix(UnitCommands __instance, UnitCommand cmd, bool fromQueue, bool doNotClearQueue, MethodBase __originalMethod)
    {
        if (!Enter(__originalMethod, out var nested)) return;
        try
        {
            var set = SetId(__instance);
            var owner = Owner(__instance)?.UniqueId;
            var c = Id(cmd);
            var prev = Id(__instance.PreviousCommand);
            var group = __instance.GroupCommand != null;
            var origin = Origin();
            Sched.Obs("uc_run", j =>
            {
                j.Prop("set", set.Value); j.Prop("u", owner); WriteId(j, "c", c); j.Prop("fq", fromQueue); j.Prop("ncq", doNotClearQueue);
                WriteId(j, "prev", prev); j.Prop("group", group); j.Prop("nested", nested); j.Prop("origin", origin);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_run", e); }
    }

    public static void RunFinalizer(UnitCommands __instance, UnitCommand cmd, Exception __exception)
    {
        if (!Pop() || !On) return;
        try
        {
            var set = SetId(__instance);
            var c = Id(cmd);
            var exc = __exception != null;
            Sched.Obs("uc_run_end", j => { j.Prop("set", set.Value); WriteId(j, "c", c); j.Prop("exc", exc); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_run_end", e); }
    }

    // UnitCommands.AddToQueueOrRun(cmd, first).
    public static void QueuePrefix(UnitCommands __instance, UnitCommand cmd, bool first, MethodBase __originalMethod)
    {
        if (!Enter(__originalMethod, out var nested)) return;
        try
        {
            var set = SetId(__instance);
            var owner = Owner(__instance)?.UniqueId;
            var c = Id(cmd);
            var origin = Origin();
            Sched.Obs("uc_queue", j => { j.Prop("set", set.Value); j.Prop("u", owner); WriteId(j, "c", c); j.Prop("first", first); j.Prop("nested", nested); j.Prop("origin", origin); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_queue", e); }
    }

    public static void QueueFinalizer(UnitCommands __instance, Exception __exception)
    {
        if (!Pop() || !On) return;
        try
        {
            var set = SetId(__instance);
            var exc = __exception != null;
            Sched.Obs("uc_queue_end", j => { j.Prop("set", set.Value); j.Prop("exc", exc); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_queue_end", e); }
    }

    // The external mutators (and RemoveFinishedAndUpdateQueue from outside the controller's tick). One
    // called by base lifecycle code (InterruptGroupCommand in Run, RemoveFinishedAndUpdateQueue in
    // TickOnUnit) is its caller's code: no records of its own.
    public static void MutatorPrefix(UnitCommands __instance, MethodBase __originalMethod, object[] __args)
    {
        if (!On) { Push(false, false); return; }
        try
        {
            var nested = Entries.Count > 0 && Chain(__originalMethod) == 1;
            if (nested) { Push(InRecordedEntry, false); return; }
            Push(true, true);
            var name = __originalMethod.Name;
            var set = SetId(__instance);
            var owner = Owner(__instance)?.UniqueId;
            bool? raise = name == nameof(UnitCommands.InterruptAll) && __args.Length == 1 && __args[0] is bool r ? r : (bool?)null;
            var pred = name == nameof(UnitCommands.InterruptAll) && __args.Length == 1 && !(__args[0] is bool);
            var origin = Origin();
            Sched.Obs("uc_x", j =>
            {
                j.Prop("fn", pred ? "InterruptAllWhere" : name); j.Prop("set", set.Value); j.Prop("u", owner); j.Prop("raise", raise);
                j.Prop("origin", origin);
            });
        }
        catch (Exception e) { Push(false, false); Trace.HookFailed("observe uc_x", e); }
    }

    public static void MutatorFinalizer(UnitCommands __instance, Exception __exception)
    {
        if (!Pop() || !On) return;
        try
        {
            var set = SetId(__instance);
            var exc = __exception != null;
            Sched.Obs("uc_x_end", j => { j.Prop("set", set.Value); j.Prop("exc", exc); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_x_end", e); }
    }

    // UnitEntityData.ReplaceCommands (UnitCommands.Temporary: the predictions' sets).
    public static void ReplacePrefix(UnitEntityData __instance, UnitCommands newCmds)
    {
        if (!On) return;
        try
        {
            var from = SetId(__instance.Commands);
            var to = SetId(newCmds);
            var sim = newCmds?.IsSimulating;
            var id = __instance.UniqueId;
            var origin = Origin();
            Sched.Obs("uc_set", j => { j.Prop("u", id); WriteId(j, "from", from); WriteId(j, "to", to); j.Prop("sim", sim); j.Prop("origin", origin); });
            // A new set's state (empty, or as the last prediction left it).
            if (newCmds != null)
            {
                var sid = to.Value;
                var row = SetRow(newCmds);
                if (!LastSet.TryGetValue(sid, out var old) || old != row)
                {
                    LastSet[sid] = row;
                    Sched.Obs("uc_state", j => { j.Prop("set", sid); j.Prop("u", (string)null); j.Prop("s", row); });
                }
            }
        }
        catch (Exception e) { Trace.HookFailed("observe uc_set", e); }
    }

    // UnitCommandController.TickOnUnit: recorded for a unit with a command, a queue or a parked command.
    public static void TickOnUnitPrefix(UnitEntityData unit)
    {
        if (!On || unit == null) { Push(false, false); return; }
        var pushed = false;
        try
        {
            var s = unit.Commands;
            var busy = s != null && (s.Raw.Any(c => c != null) || s.Queue.Count > 0 || s.PreviousCommand != null);
            Push(busy, busy);
            pushed = true;
            if (!busy) return;
            var set = SetId(s);
            var id = unit.UniqueId;
            var view = unit.View;
            var hasView = view != null;
            var rb = hasView && (bool)view.RigidbodyController && view.RigidbodyController.IsControllingRigidbody;
            Sched.Obs("uc_tick", j => { j.Prop("u", id); j.Prop("set", set.Value); j.Prop("view", hasView); j.Prop("rb", rb); });
        }
        catch (Exception e)
        {
            if (!pushed) Push(false, false);
            Trace.HookFailed("observe uc_tick", e);
        }
    }

    public static void TickOnUnitFinalizer(UnitEntityData unit)
    {
        if (!Pop() || !On || unit == null) return;
        try
        {
            var id = unit.UniqueId;
            Sched.Obs("uc_tick_end", j => j.Prop("u", id));
        }
        catch (Exception e) { Trace.HookFailed("observe uc_tick_end", e); }
    }

    // UnitCommandController.TickCommand(command, canMoveUnit).
    public static void TickCommandPrefix(UnitCommand command, bool canMoveUnit)
    {
        if (!On || !InRecordedEntry) return;
        try
        {
            var c = Id(command);
            Sched.Obs("uc_tc", j => { WriteId(j, "c", c); j.Prop("canmove", canMoveUnit); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_tc", e); }
    }

    public static void TickCommandFinalizer(UnitCommand command, Exception __exception)
    {
        if (!On || !InRecordedEntry || __exception == null) return;
        try
        {
            var c = Id(command);
            var msg = __exception.GetType().Name + ": " + __exception.Message;
            Sched.Obs("uc_exc", j => { WriteId(j, "c", c); j.Prop("msg", msg); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_exc", e); }
    }

    // ---- leaves ----

    // The leaf methods by handle: their record name.
    private static readonly Dictionary<IntPtr, string> LeafNames = new Dictionary<IntPtr, string>();

    private static bool Leaf(MethodBase m, out string name, bool sys = false)
    {
        name = null;
        if (!On || !InRecordedEntry) return false;
        if (!LeafNames.TryGetValue(m.MethodHandle.Value, out name)) return false;
        return Chain(m, sys) == 1;
    }

    private static void Emit(string f, Action<Json> body)
    {
        Sched.Obs("uc_v", j => { j.Prop("f", f); body(j); });
    }

    // A command's boolean answer (IsUnitCloseEnough, CannotStop, the class's getters).
    public static void CommandBoolPostfix(UnitCommand __instance, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var c = Id(__instance);
            Emit(f, j => { WriteId(j, "c", c); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // TryMergeInto(currentCommand): its result, and the current command's target after it.
    public static void MergePostfix(UnitCommand __instance, UnitCommand currentCommand, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var c = Id(__instance);
            var into = Id(currentCommand);
            var tgt = currentCommand == null ? null : TargetStr(currentCommand.Target);
            Emit(f, j => { WriteId(j, "c", c); WriteId(j, "into", into); j.Prop("v", __result); j.Prop("tgt", tgt); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // A unit's boolean answer (the view's IsMoving and IsGetUp, the hands, IsDirectlyControllable, ...).
    public static void UnitBoolPostfix(object __instance, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var u = UnitOf(__instance);
            Emit(f, j => { j.Prop("u", u); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    private static string UnitOf(object o)
    {
        switch (o)
        {
            case UnitEntityData u: return u.UniqueId;
            case UnitEntityView v: return v.EntityData?.UniqueId;
            case UnitMovementAgentBase a: return a.Unit != null ? a.Unit.EntityData?.UniqueId : null;
            case EntityDataBase e: return e.UniqueId;
            case Kingmaker.Controllers.Combat.UnitCombatState cs: return cs.Unit?.UniqueId;
            case Kingmaker.UnitLogic.UnitState st: return st.Owner?.Unit?.UniqueId;
            default: return null;
        }
    }

    // Static and argument-taking leaves: a unit argument and the result.
    public static void UnitArgPostfix(UnitEntityData unit, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var u = unit?.UniqueId;
            Emit(f, j => { j.Prop("u", u); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // UnitCommand.CommandTargetUntargetable(sourceEntity, targetUnit, evt).
    public static void UntargetablePostfix(EntityDataBase sourceEntity, UnitEntityData targetUnit, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var s = sourceEntity?.UniqueId;
            var u = targetUnit?.UniqueId;
            Emit(f, j => { j.Prop("src", s); j.Prop("u", u); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // UnitEntityData.IsEnemy(unit).
    public static void EnemyPostfix(UnitEntityData __instance, UnitEntityData unit, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var a = __instance.UniqueId;
            var b = unit?.UniqueId;
            Emit(f, j => { j.Prop("u", a); j.Prop("o", b); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // UnitStealthExtension.InStealthFor(unit, observer).
    public static void StealthPostfix(UnitEntityData unit, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var a = unit?.UniqueId;
            Emit(f, j => { j.Prop("u", a); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // UnitHelper.GetSaddledUnit(unit) (a mounted target in Run).
    public static void SaddledPostfix(UnitEntityData unit, UnitEntityData __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var u = unit?.UniqueId;
            var r = __result?.UniqueId;
            Emit(f, j => { j.Prop("u", u); j.Prop("v", r); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // UnitCommands.TryUseCustomHandler(cmd, fromQueue).
    public static void CustomPostfix(UnitCommand cmd, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var c = Id(cmd);
            Emit(f, j => { WriteId(j, "c", c); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // TargetWrapper.Equals(object) (TryAddToQueueInsteadOfRunImmediately's target comparison).
    public static void TargetEqualsPostfix(TargetWrapper __instance, object obj, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f)) return;
            var a = TargetStr(__instance);
            var b = TargetStr(obj as TargetWrapper);
            Emit(f, j => { j.Prop("a", a); j.Prop("b", b); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // ---- class code ----

    // Whether each class-code call on the stack is the lifecycle's own (innermost last).
    private static readonly List<bool> _fx = new List<bool>();

    // The commands of those calls (the taps' command).
    private static readonly List<UnitCommand> _fxCmd = new List<UnitCommand>();

    public static void FxPrefix(UnitCommand __instance, MethodBase __originalMethod)
    {
        var mine = false;
        try
        {
            mine = On && InRecordedEntry && Chain(__originalMethod) == 1;
            // A base implementation called by an override (base.Init() in UnitAttack's, UnitAttackOfOpportunity's
            // Init): the override's code, not a class-code call of its own.
            if (mine && !MostDerived(__instance.GetType(), __originalMethod)) mine = false;
        }
        catch (Exception e) { Trace.HookFailed("observe uc_fx", e); }
        _fx.Add(mine);
        _fxCmd.Add(__instance);
    }

    // Whether `m` is the implementation `t` runs (no class between `t` and m's declaring type overrides it).
    private static readonly Dictionary<(Type, IntPtr), bool> _mostDerived = new Dictionary<(Type, IntPtr), bool>();

    private static bool MostDerived(Type t, MethodBase m)
    {
        var key = (t, m.MethodHandle.Value);
        if (_mostDerived.TryGetValue(key, out var v)) return v;
        var ps = m.GetParameters().Select(p => p.ParameterType).ToArray();
        v = true;
        for (var x = t; x != null && x != m.DeclaringType; x = x.BaseType)
            if (x.GetMethod(m.Name, BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly, null, ps, null) != null)
            {
                v = false;
                break;
            }
        _mostDerived[key] = v;
        return v;
    }

    private static void FxEnd(UnitCommand c, MethodBase m, int? ret, Exception exception)
    {
        var mine = _fx.Count > 0 && _fx[_fx.Count - 1];
        if (_fx.Count > 0) _fx.RemoveAt(_fx.Count - 1);
        if (_fxCmd.Count > 0) _fxCmd.RemoveAt(_fxCmd.Count - 1);
        if (!mine || !On) return;
        try
        {
            var id = Id(c);
            var fn = m.Name;
            var row = Row(c);
            var exc = exception != null;
            var ua = AttacksOn && c is UnitAttack a ? AttackRow(a) : null;
            Sched.Obs("uc_fx", j =>
            {
                j.Prop("fn", fn); WriteId(j, "c", id); WriteId(j, "ret", ret); j.Prop("s", row); j.Prop("exc", exc);
                if (ua != null) j.Prop("ua", ua);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_fx", e); }
    }

    public static void FxFinalizer(UnitCommand __instance, MethodBase __originalMethod, Exception __exception) =>
        FxEnd(__instance, __originalMethod, null, __exception);

    public static void FxActionFinalizer(UnitCommand __instance, MethodBase __originalMethod, UnitCommand.ResultType __result, Exception __exception) =>
        FxEnd(__instance, __originalMethod, __exception == null ? (int)__result : (int?)null, __exception);

    // ---- UnitAttack (observer "attacks") ----

    // Whether UnitAttack's code is lifecycle code (Install sets it; the trace's begin lists the observer).
    internal static bool AttacksOn;

    private static readonly AccessTools.FieldRef<UnitAttack, List<AttackHandInfo>> AllAttacksF =
        AccessTools.FieldRefAccess<UnitAttack, List<AttackHandInfo>>("m_AllAttacks");
    private static readonly AccessTools.FieldRef<UnitAttack, int> AttackIndexF =
        AccessTools.FieldRefAccess<UnitAttack, int>("m_AttackIndex");
    private static readonly AccessTools.FieldRef<UnitAttack, bool> HasStartedAttackF =
        AccessTools.FieldRefAccess<UnitAttack, bool>("m_HasStartedAttack");
    private static readonly AccessTools.FieldRef<UnitAttack, bool> ClearOnFinishF =
        AccessTools.FieldRefAccess<UnitAttack, bool>("m_ClearAnimationOnFinish");
    private static readonly AccessTools.FieldRef<UnitAttack, float> DurationF =
        AccessTools.FieldRefAccess<UnitAttack, float>("m_AnimationsDuration");
    private static readonly AccessTools.FieldRef<UnitAttack, bool> UnconsciousAtStartF =
        AccessTools.FieldRefAccess<UnitAttack, bool>("m_IsUnconsciousAtStartOfCommand");

    // "attackIndex|hasStartedAttack clearOnFinish IsAttackFull unconsciousAtStart|m_AnimationsDuration|
    //  LastAttackRule.AttackNumber ('-' without one)|the list's length ('-' without one)".
    private static string AttackRow(UnitAttack a)
    {
        var list = AllAttacksF(a);
        var last = a.LastAttackRule;
        return AttackIndexF(a).ToString(CultureInfo.InvariantCulture) + "|" + B(HasStartedAttackF(a)) + B(ClearOnFinishF(a)) + B(a.IsAttackFull) +
               B(UnconsciousAtStartF(a)) + "|" + DurationF(a).ToString("R", CultureInfo.InvariantCulture) + "|" +
               (last == null ? "-" : last.AttackNumber.ToString(CultureInfo.InvariantCulture)) + "|" +
               (list == null ? "-" : list.Count.ToString(CultureInfo.InvariantCulture));
    }

    private static readonly AccessTools.FieldRef<Kingmaker.Items.Slots.ItemSlot, EntityRef<ItemEntity>> SlotItemRef =
        AccessTools.FieldRefAccess<Kingmaker.Items.Slots.ItemSlot, EntityRef<ItemEntity>>("m_ItemRef");
    private static readonly AccessTools.FieldRef<Kingmaker.Items.Slots.ItemSlot, bool> SlotActive =
        AccessTools.FieldRefAccess<Kingmaker.Items.Slots.ItemSlot, bool>("m_Active");
    private static readonly AccessTools.FieldRef<Kingmaker.Items.Slots.ItemSlot, bool> SlotFake =
        AccessTools.FieldRefAccess<Kingmaker.Items.Slots.ItemSlot, bool>("m_IsItemFake");

    // An attack as ua_atk has it: "slot,item,active,fake,penalty,flags": the slot ("p" primary, "s" secondary,
    // "l<i>" an additional limb, "?" another), its fields (the item reference's id, never resolved: empty without
    // one; m_Active; m_IsItemFake), and three flags: a handle, IsSequential, and OnAction's skip test (no handle,
    // or Variant < 0 without a clip). The body's slots are read through its fields' getters (no reference resolved).
    private static string AttackStr(AttackHandInfo x, Kingmaker.Items.UnitBody body)
    {
        var slot = "?";
        if (body != null && x.Hand != null)
        {
            if (ReferenceEquals(x.Hand, body.PrimaryHand)) slot = "p";
            else if (ReferenceEquals(x.Hand, body.SecondaryHand)) slot = "s";
            else
            {
                var i = body.AdditionalLimbs.IndexOf(x.Hand);
                if (i >= 0) slot = "l" + i.ToString(CultureInfo.InvariantCulture);
            }
        }
        var item = x.Hand == null ? "" : SlotItemRef(x.Hand).Id ?? "";
        var active = x.Hand != null && SlotActive(x.Hand);
        var fake = x.Hand != null && SlotFake(x.Hand);
        var h = x.AnimationHandle;
        var skip = h == null || (h.Variant < 0 && h.AnimationClipWrapper == null);
        return slot + "," + item + "," + B(active) + "," + B(fake) + "," +
               x.AttackBonusPenalty.ToString(CultureInfo.InvariantCulture) + "," + B(h != null) + B(x.IsSequential) + B(skip);
    }

    private static string AttackFlags(UnitAttack a) =>
        B(a.IsSingleAttack) + B(a.ForceFullAttack) + B(a.IsCharge) + B(a.OnlyForInspect) + B(a.OverrideAction != null);

    // InitAttacks, when it's the lifecycle's (innermost last), and whether it called ConfigureAnimations (its gate:
    // the view has an animation manager).
    private static readonly List<bool> _atk = new List<bool>();
    private static readonly List<bool> _atkCfg = new List<bool>();

    public static void InitAttacksPrefix(MethodBase __originalMethod)
    {
        var mine = false;
        try { mine = On && AttacksOn && InRecordedEntry && Chain(__originalMethod) == 1; }
        catch (Exception e) { Trace.HookFailed("observe ua_atk", e); }
        _atk.Add(mine);
        _atkCfg.Add(false);
    }

    public static void ConfigureAnimationsPrefix()
    {
        if (_atkCfg.Count > 0) _atkCfg[_atkCfg.Count - 1] = true;
    }

    public static void InitAttacksFinalizer(UnitAttack __instance, Exception __exception)
    {
        var mine = _atk.Count > 0 && _atk[_atk.Count - 1];
        if (_atk.Count > 0) _atk.RemoveAt(_atk.Count - 1);
        var cfg = _atkCfg.Count > 0 && _atkCfg[_atkCfg.Count - 1];
        if (_atkCfg.Count > 0) _atkCfg.RemoveAt(_atkCfg.Count - 1);
        if (!mine || !On) return;
        try
        {
            var c = Id(__instance);
            var flags = AttackFlags(__instance);
            var full = __instance.IsAttackFull;
            var dur = DurationF(__instance);
            var list = AllAttacksF(__instance);
            var body = __instance.Executor?.Body;
            var attacks = list?.Select(x => x == null ? null : AttackStr(x, body)).ToList();
            var exc = __exception != null;
            Sched.Obs("ua_atk", j =>
            {
                WriteId(j, "c", c); j.Prop("flags", flags); j.Prop("full", full); j.Prop("cfg", cfg);
                j.Prop("dur", dur.ToString("R", CultureInfo.InvariantCulture));
                j.Key("list");
                if (attacks == null) j.W.WriteNull();
                else { j.W.WriteStartArray(); foreach (var a in attacks) j.W.WriteValue(a); j.W.WriteEndArray(); }
                j.Prop("exc", exc);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ua_atk", e); }
    }

    // UnitAttack.IsUnitInFiveFeetRange(unit) (positions, corpulences, the weapon's range); the walk passes
    // UpdateTarget's Where.
    public static void FivePostfix(UnitAttack __instance, UnitEntityData unit, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!Leaf(__originalMethod, out var f, true)) return;
            var c = Id(__instance);
            var u = unit?.UniqueId;
            Emit(f, j => { WriteId(j, "c", c); j.Prop("u", u); j.Prop("v", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v", e); }
    }

    // Taps: reads UnitAttack makes through getters the JIT inlines, replaced in its IL by these (the same call,
    // then a uc_v record when the read is the lifecycle's), about the innermost class-code call's command.
    private static void Tap(string f, Action<Json> body)
    {
        try
        {
            if (!On || !AttacksOn || !InRecordedEntry || _fxCmd.Count == 0 || Chain(null, true) != 1) return;
            var c = Id(_fxCmd[_fxCmd.Count - 1]);
            Emit(f, j => { WriteId(j, "c", c); body(j); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v " + f, e); }
    }

    public static bool TapFinished(AnimationActionHandle h)
    {
        var v = h.IsFinished;
        Tap("afin", j => j.Prop("v", v));
        return v;
    }

    public static bool TapReleased(AnimationActionHandle h)
    {
        var v = h.IsReleased;
        Tap("arel", j => j.Prop("v", v));
        return v;
    }

    public static Kingmaker.View.Animation.WeaponAnimationStyle TapStyle(ItemEntityWeapon w, bool forDollRoom)
    {
        var v = w.GetAnimationStyle(forDollRoom);
        Tap("style", j => j.Prop("v", (int)v));
        return v;
    }

    // Memory.Enemies: the units' ids (UnitReference.UniqueId, never Value, which resolves a proxy).
    public static List<UnitGroupMemory.UnitInfo> TapEnemies(UnitGroupMemory m)
    {
        var v = m.Enemies;
        Tap("enemies", j =>
        {
            j.Key("v");
            if (v == null) { j.W.WriteNull(); return; }
            j.W.WriteStartArray();
            foreach (var x in v) { if (x == null) j.W.WriteNull(); else j.W.WriteValue(x.UnitReference.UniqueId ?? ""); }
            j.W.WriteEndArray();
        });
        return v;
    }

    public static void TapSort(List<UnitEntityData> l, Comparison<UnitEntityData> cmp)
    {
        l.Sort(cmp);
        Tap("sorted", j =>
        {
            j.Key("v"); j.W.WriteStartArray();
            foreach (var u in l) { if (u == null) j.W.WriteNull(); else j.W.WriteValue(u.UniqueId); }
            j.W.WriteEndArray();
        });
    }

    // The taps by the call they replace (declaring type, name, parameter types).
    private static readonly List<(Type type, string name, Type[] args, string tap)> TapSites = new List<(Type, string, Type[], string)>
    {
        (typeof(AnimationActionHandle), "get_IsFinished", Type.EmptyTypes, nameof(TapFinished)),
        (typeof(AnimationActionHandle), "get_IsReleased", Type.EmptyTypes, nameof(TapReleased)),
        (typeof(ItemEntityWeapon), nameof(ItemEntityWeapon.GetAnimationStyle), new[] { typeof(bool) }, nameof(TapStyle)),
        (typeof(UnitGroupMemory), "get_Enemies", Type.EmptyTypes, nameof(TapEnemies)),
        (typeof(List<UnitEntityData>), nameof(List<UnitEntityData>.Sort), new[] { typeof(Comparison<UnitEntityData>) }, nameof(TapSort)),
    };

    // Replacements made, by "method:tap".
    internal static readonly Dictionary<string, int> Tapped = new Dictionary<string, int>();

    public static IEnumerable<CodeInstruction> AttackTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodBase mb)
            {
                foreach (var (type, name, args, tap) in TapSites)
                {
                    if (mb.DeclaringType != type || mb.Name != name || !mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(args)) continue;
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), tap);
                    var k = original.Name + ":" + tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            yield return i;
        }
    }


    // ---- the approach (observer "approach") ----

    // Whether TickApproaching, UnitMoveTo's and UnitAttackOfOpportunity's code are lifecycle code (Install sets it).
    internal static bool ApproachOn;

    private static readonly AccessTools.FieldRef<UnitCommand, float> MaxApproachRadiusF =
        AccessTools.FieldRefAccess<UnitCommand, float>("m_MaxApproachRadius");
    private static readonly PropertyInfo DistanceXZP = AccessTools.Property(typeof(UnitCommand), "DistanceXZ");
    private static readonly AccessTools.FieldRef<UnitMoveTo, bool?> OverrideAwaitF =
        AccessTools.FieldRefAccess<UnitMoveTo, bool?>("m_OverrideAwaitMovementFinish");
    private static readonly AccessTools.FieldRef<UnitMoveTo, bool> UseRiderTargetF =
        AccessTools.FieldRefAccess<UnitMoveTo, bool>("m_UseRiderTarget");
    private static readonly AccessTools.FieldRef<UnitAttackOfOpportunity, bool> AooTurnF =
        AccessTools.FieldRefAccess<UnitAttackOfOpportunity, bool>("m_ShouldTurnToTarget");

    private static string F(float v) => v.ToString("R", CultureInfo.InvariantCulture);

    // "ApproachRadius|m_MaxApproachRadius|DistanceXZ NeedLoS".
    private static string ApproachStr(UnitCommand c) =>
        F(c.ApproachRadius) + "|" + F(MaxApproachRadiusF(c)) + "|" + B((bool)DistanceXZP.GetValue(c)) + B(c.NeedLoS);

    // "Roaming PreventInterrupt UseRiderTarget|Orientation ('-' without one)|the await override ('-', 0, 1)|MovementDelay".
    private static string MoveToStr(UnitMoveTo m)
    {
        var o = OverrideAwaitF(m);
        return B(m.Roaming) + B(m.PreventInterrupt) + B(UseRiderTargetF(m)) + "|" + (m.Orientation.HasValue ? F(m.Orientation.Value) : "-") + "|" +
               (o.HasValue ? B(o.Value) : "-") + "|" + F(m.MovementDelay);
    }

    private static bool AooTurn(UnitAttackOfOpportunity a) => AooTurnF(a);

    // TickApproaching (base code): its command, for the taps and calls inside it.
    public static void ApproachPrefix(UnitCommand __instance)
    {
        _fx.Add(false);
        _fxCmd.Add(__instance);
    }

    public static void ApproachFinalizer()
    {
        if (_fx.Count > 0) _fx.RemoveAt(_fx.Count - 1);
        if (_fxCmd.Count > 0) _fxCmd.RemoveAt(_fxCmd.Count - 1);
    }

    private static void ApTap(string f, Action<Json> body)
    {
        try
        {
            if (!On || !ApproachOn || !InRecordedEntry || _fxCmd.Count == 0 || Chain(null) != 1) return;
            var c = Id(_fxCmd[_fxCmd.Count - 1]);
            Emit(f, j => { WriteId(j, "c", c); body(j); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v " + f, e); }
    }

    // A path's points as the agent gets them: null, or [[x,y,z], ...] (round-trip floats).
    private static void WritePath(Json j, Pathfinding.Path p)
    {
        j.Key("v");
        var pts = p?.vectorPath;
        if (p == null) { j.W.WriteNull(); return; }
        j.W.WriteStartArray();
        if (pts != null) foreach (var x in pts) j.Vec(x);
        j.W.WriteEndArray();
    }

    public static Kingmaker.UnitLogic.Parts.UnitPartSaddled TapSaddledPart(UnitEntityData u)
    {
        var v = u.SaddledPart;
        var b = v != null;
        ApTap("spart", j => j.Prop("v", b));
        return v;
    }

    public static Kingmaker.UnitLogic.Parts.UnitPartRider TapRiderPart(UnitEntityData u)
    {
        var v = u.RiderPart;
        var b = v != null;
        ApTap("rpart", j => j.Prop("v", b));
        return v;
    }

    public static bool TapTactical()
    {
        var v = Kingmaker.Armies.TacticalCombat.TacticalCombatHelper.IsActive;
        ApTap("tact", j => j.Prop("v", v));
        return v;
    }

    public static bool TapWants(UnitMovementAgentBase a)
    {
        var v = a.WantsToMove;
        ApTap("wants", j => j.Prop("v", v));
        return v;
    }

    public static bool TapHasMovement(TurnController t, UnitEntityData u)
    {
        var v = t.HasMovement(u);
        ApTap("hasmove", j => j.Prop("v", v));
        return v;
    }

    public static bool TapCanMove(Kingmaker.UnitLogic.UnitState st)
    {
        var v = st.CanMove;
        ApTap("canmove", j => j.Prop("v", v));
        return v;
    }

    public static bool TapUcgu(UniRx.ReactiveProperty<bool> p)
    {
        var v = p.Value;
        ApTap("ucgu", j => j.Prop("v", v));
        return v;
    }

    public static Pathfinding.Path TapForcedPath(UnitCommand c)
    {
        var v = c.ForcedPath;
        ApTap("fpath", j => WritePath(j, v));
        return v;
    }

    public static Kingmaker.TurnBasedMode.PathVisualizer TapPathVisualizer()
    {
        var v = Kingmaker.TurnBasedMode.PathVisualizer.Instance;
        // `?.`: a reference test, not Unity's.
        var b = !ReferenceEquals(v, null);
        ApTap("pvis", j => j.Prop("v", b));
        return v;
    }

    public static Pathfinding.Path TapCurrentPath(Kingmaker.TurnBasedMode.PathVisualizer p, UnitEntityView view)
    {
        var v = p.CurrentPathForUnit(view);
        ApTap("vpath", j => WritePath(j, v));
        return v;
    }

    // `targetUnit.View.Or(null)` in TickApproaching's MoveTo: whether the target's view is alive (Unity's bool).
    public static UnitEntityView TapViewOr(UnitEntityView view, UnitEntityView valueIfNull)
    {
        var r = Owlcat.Runtime.Core.Utils.ObjectExtensions.Or(view, valueIfNull);
        bool alive = view;
        ApTap("tview", j => j.Prop("v", alive));
        return r;
    }

    public static bool TapPreventMovementFinish(Kingmaker.AI.AiAction a)
    {
        var v = a.PreventMovementFinish;
        ApTap("pmf", j => j.Prop("v", v));
        return v;
    }

    public static Kingmaker.Items.Slots.WeaponSlot TapThreatHand(UnitEntityData u)
    {
        var v = u.GetThreatHand();
        var body = u.Body;
        var s = v == null ? null : SlotStr(v, body);
        ApTap("threat", j => j.Prop("v", s));
        return v;
    }

    public static Kingmaker.Visual.Animation.Kingmaker.UnitAnimationManager TapAnimationManager(UnitEntityView view)
    {
        var v = view.AnimationManager;
        // `== null`: Unity's (a destroyed manager is null).
        var b = v != null;
        ApTap("anim", j => j.Prop("v", b));
        return v;
    }

    public static bool TapIdle(Kingmaker.Visual.Animation.Kingmaker.UnitAnimationManager m)
    {
        var v = m.CanRunIdleAction();
        ApTap("idle", j => j.Prop("v", v));
        return v;
    }

    // A slot as ua_atk writes it: "slot,item,active,fake" (slot "p", "s", "l<i>" or "?").
    private static string SlotStr(Kingmaker.Items.Slots.WeaponSlot hand, Kingmaker.Items.UnitBody body)
    {
        var slot = "?";
        if (body != null)
        {
            if (ReferenceEquals(hand, body.PrimaryHand)) slot = "p";
            else if (ReferenceEquals(hand, body.SecondaryHand)) slot = "s";
            else
            {
                var i = body.AdditionalLimbs.IndexOf(hand);
                if (i >= 0) slot = "l" + i.ToString(CultureInfo.InvariantCulture);
            }
        }
        return slot + "," + (SlotItemRef(hand).Id ?? "") + "," + B(SlotActive(hand)) + "," + B(SlotFake(hand));
    }

    // The lifecycle's calls into the movement agent (from base code only).
    private static bool Called(MethodBase m) => On && ApproachOn && InRecordedEntry && Chain(m) == 1;

    private static int? CallCmd() => _fxCmd.Count == 0 ? null : Id(_fxCmd[_fxCmd.Count - 1]);

    // UnitEntityView.MoveTo(command, point, approachRadius, maxApproachRadius, targetUnit, distanceXZ).
    public static void MoveToPrefix(UnitEntityView __instance, UnitCommand command, UnityEngine.Vector3 point, float approachRadius,
        float maxApproachRadius, UnitEntityView targetUnit, bool distanceXZ, MethodBase __originalMethod)
    {
        try
        {
            if (!Called(__originalMethod)) return;
            var u = UnitOf(__instance);
            var c = Id(command);
            var t = targetUnit == null ? null : UnitOf(targetUnit);
            var tv = !ReferenceEquals(targetUnit, null);
            Sched.Obs("uc_call", j =>
            {
                j.Prop("fn", "moveto"); j.Prop("u", u); WriteId(j, "c", c); j.Prop("p", point); j.Prop("r", F(approachRadius));
                j.Prop("mr", F(maxApproachRadius)); j.Prop("tv", tv); j.Prop("tu", t); j.Prop("xz", distanceXZ);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_call", e); }
    }

    // UnitMovementAgent.FollowPrecomputedPath(p, forcedPath, maxApproachRadius).
    public static void FollowPrefix(UnitMovementAgent __instance, Pathfinding.Path p, bool forcedPath, float maxApproachRadius, MethodBase __originalMethod)
    {
        try
        {
            if (!Called(__originalMethod)) return;
            var u = UnitOf(__instance);
            var c = CallCmd();
            Sched.Obs("uc_call", j =>
            {
                j.Prop("fn", "follow"); j.Prop("u", u); WriteId(j, "c", c); WritePath(j, p); j.Prop("forced", forcedPath);
                j.Prop("r", F(maxApproachRadius));
            });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_call", e); }
    }

    // UnitEntityView.StopMoving().
    public static void StopPrefix(UnitEntityView __instance, MethodBase __originalMethod)
    {
        try
        {
            if (!Called(__originalMethod)) return;
            var u = UnitOf(__instance);
            var c = CallCmd();
            Sched.Obs("uc_call", j => { j.Prop("fn", "stop"); j.Prop("u", u); WriteId(j, "c", c); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_call", e); }
    }

    // The approach's taps by the call they replace (declaring type, name, parameter types), per method; `first`: only the
    // method's first such call.
    private static readonly List<(string method, Type type, string name, Type[] args, string tap, bool first)> ApproachTapSites =
        new List<(string, Type, string, Type[], string, bool)>
        {
            ("TickApproaching", typeof(UnitEntityData), "get_SaddledPart", Type.EmptyTypes, nameof(TapSaddledPart), false),
            ("TickApproaching", typeof(UnitEntityData), "get_RiderPart", Type.EmptyTypes, nameof(TapRiderPart), false),
            ("TickApproaching", typeof(Kingmaker.Armies.TacticalCombat.TacticalCombatHelper), "get_IsActive", Type.EmptyTypes, nameof(TapTactical), false),
            ("TickApproaching", typeof(UnitMovementAgentBase), "get_WantsToMove", Type.EmptyTypes, nameof(TapWants), false),
            ("TickApproaching", typeof(TurnController), nameof(TurnController.HasMovement), new[] { typeof(UnitEntityData) }, nameof(TapHasMovement), false),
            ("TickApproaching", typeof(Kingmaker.UnitLogic.UnitState), "get_CanMove", Type.EmptyTypes, nameof(TapCanMove), false),
            ("TickApproaching", typeof(UniRx.ReactiveProperty<bool>), "get_Value", Type.EmptyTypes, nameof(TapUcgu), false),
            ("TickApproaching", typeof(UnitCommand), "get_ForcedPath", Type.EmptyTypes, nameof(TapForcedPath), false),
            ("TickApproaching", typeof(Kingmaker.TurnBasedMode.PathVisualizer), "get_Instance", Type.EmptyTypes, nameof(TapPathVisualizer), false),
            ("TickApproaching", typeof(Kingmaker.TurnBasedMode.PathVisualizer), nameof(Kingmaker.TurnBasedMode.PathVisualizer.CurrentPathForUnit), new[] { typeof(UnitEntityView) }, nameof(TapCurrentPath), false),
            ("TickApproaching", typeof(Owlcat.Runtime.Core.Utils.ObjectExtensions), "Or", new[] { typeof(UnitEntityView), typeof(UnitEntityView) }, nameof(TapViewOr), false),
            ("OnAction", typeof(Kingmaker.AI.AiAction), "get_PreventMovementFinish", Type.EmptyTypes, nameof(TapPreventMovementFinish), false),
            ("Init", typeof(Kingmaker.UnitLogic.UnitHelper), nameof(Kingmaker.UnitLogic.UnitHelper.GetThreatHand), new[] { typeof(UnitEntityData) }, nameof(TapThreatHand), false),
            ("OnRun", typeof(UnitEntityView), "get_AnimationManager", Type.EmptyTypes, nameof(TapAnimationManager), true),
            ("OnRun", typeof(Kingmaker.Visual.Animation.Kingmaker.UnitAnimationManager), nameof(Kingmaker.Visual.Animation.Kingmaker.UnitAnimationManager.CanRunIdleAction), Type.EmptyTypes, nameof(TapIdle), false),
        };

    public static IEnumerable<CodeInstruction> ApproachTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        var done = new HashSet<string>();
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodBase mb)
            {
                foreach (var (method, type, name, args, tap, first) in ApproachTapSites)
                {
                    if (method != original.Name || mb.DeclaringType != type || mb.Name != name ||
                        !mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(args)) continue;
                    if (first && done.Contains(tap)) break;
                    done.Add(tap);
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), tap);
                    var k = original.DeclaringType?.Name + "." + original.Name + ":" + tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            yield return i;
        }
    }

    // ---- ends and the running list ----

    public static void EndedPrefix(UnitCommand __instance, out bool __state) => __state = __instance.IsFinished;

    public static void EndedPostfix(UnitCommand __instance, bool raiseEvent, bool __state, MethodBase __originalMethod)
    {
        if (!On || __state || !__instance.IsFinished) return;
        try
        {
            var c = Id(__instance);
            var res = (int)__instance.Result;
            var ctx = InRecordedEntry && Chain(__originalMethod) == 1 ? "base" : "other";
            var origin = ctx == "base" ? null : Origin();
            Sched.Obs("uc_end", j => { WriteId(j, "c", c); j.Prop("res", res); j.Prop("raise", raiseEvent); j.Prop("ctx", ctx); j.Prop("origin", origin); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_end", e); }
    }

    public static void RunningPrefix(TurnController __instance, out int __state) => __state = Running(__instance).Count;

    public static void RunningPostfix(TurnController __instance, UnitCommand command, int __state, MethodBase __originalMethod)
    {
        if (!On) return;
        try
        {
            var c = Id(command);
            var op = __originalMethod.Name == nameof(TurnController.AddRunningCommand) ? "add" : "remove";
            var n = Running(__instance).Count;
            var current = ReferenceEquals(__instance, Game.Instance.TurnBasedCombatController?.CurrentTurn);
            var ctx = InRecordedEntry && Chain(__originalMethod) == 1 ? "base" : "other";
            Sched.Obs("uc_rc", j => { WriteId(j, "c", c); j.Prop("op", op); j.Prop("b", __state); j.Prop("a", n); j.Prop("current", current); j.Prop("ctx", ctx); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_rc", e); }
    }

    // ---- install ----

    public static void Install(Harmony h)
    {
        void P(MethodBase m, string prefix = null, string postfix = null, string finalizer = null)
        {
            if (m == null) { Health.Failed("command observe hook target missing", new Exception(prefix ?? postfix ?? finalizer)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(CommandObs), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(CommandObs), postfix),
                    finalizer: finalizer == null ? null : new HarmonyMethod(typeof(CommandObs), finalizer));
            }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e); }
        }

        var uc = typeof(UnitCommand);
        var ucs = typeof(UnitCommands);
        var ctl = typeof(UnitCommandController);
        var tc = typeof(TurnController);
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        // The method declared on `t` with that name: the only one, or the overload with those parameters.
        MethodBase M(Type t, string name, params Type[] args)
        {
            if (args.Length > 0) return t.GetMethod(name, all, null, args, null);
            var ms = t.GetMethods(all).Where(m => m.Name == name).ToList();
            if (ms.Count > 1) Health.Failed("command observe: an overloaded method named without its parameters", new Exception(t.Name + "." + name));
            return ms.FirstOrDefault();
        }
        MethodBase M0(Type t, string name) => t.GetMethod(name, all, null, Type.EmptyTypes, null);
        IntPtr H(MethodBase m) => m?.MethodHandle.Value ?? IntPtr.Zero;
        var subclasses = uc.Assembly.GetTypes().Where(t => t.IsSubclassOf(uc)).ToList();
        // A virtual member on UnitCommand and every override of it.
        IEnumerable<MethodBase> Virtual(string name)
        {
            var b = M(uc, name);
            if (b != null && !b.IsAbstract) yield return b;
            foreach (var t in subclasses)
            {
                var o = M(t, name);
                if (o != null && !o.IsAbstract) yield return o;
            }
        }

        // Entries.
        var runPrivate = M(ucs, "Run", typeof(UnitCommand), typeof(bool), typeof(bool));
        var queue = M(ucs, "AddToQueueOrRun");
        var tickOnUnit = M(ctl, "TickOnUnit");
        var mutators = new List<MethodBase>
        {
            M(ucs, nameof(UnitCommands.InterruptAll), typeof(bool)),
            M(ucs, nameof(UnitCommands.InterruptAll), typeof(Func<UnitCommand, bool>)),
            M(ucs, nameof(UnitCommands.InterruptAiCommands)),
            M(ucs, nameof(UnitCommands.InterruptMove)),
            M(ucs, nameof(UnitCommands.InterruptGroupCommand)),
            M(ucs, nameof(UnitCommands.RemoveNoStartStadartCommand)),
            M(ucs, nameof(UnitCommands.RemoveFinishedAndUpdateQueue)),
        };
        foreach (var m in new[] { runPrivate, queue, tickOnUnit }.Concat(mutators)) EntrySet.Add(H(m));

        // Base lifecycle code.
        foreach (var m in new[]
                 {
                     M(ctl, "TickCommandTurnBased"), M(ctl, "TickCommand"), M(ctl, "ShouldStartCommand"),
                     M(ucs, "Run", typeof(UnitCommand)), M(ucs, "CanRunCommand"), M(ucs, "FixTargetIfTargetOnMount"), M(ucs, "GetPaired"),
                     M(ucs, "TryUseSpecialHandlers"), M(ucs, "TryAddToQueueInsteadOfRunImmediately"), M(ucs, "AddToQueueInternal"),
                     M(ucs, nameof(UnitCommands.AddToQueue)), M(ucs, nameof(UnitCommands.AddToQueueFirst)), M(ucs, "HasUniterrapitble"),
                     M(ucs, "InterruptAndRemoveCommand", typeof(UnitCommand.CommandType)),
                     M(ucs, "InterruptAndRemoveCommand", typeof(UnitCommand.CommandType), typeof(bool)),
                     M(ucs, nameof(UnitCommands.GetCommand), typeof(UnitCommand.CommandType)), M(ucs, "get_Standard"), M(ucs, "get_Swift"),
                     M(ucs, "get_Free"), M(ucs, "get_Move"), M(ucs, "get_Empty"), M(ucs, nameof(UnitCommands.IsRunning)),
                     M(uc, nameof(UnitCommand.Start)), M(uc, nameof(UnitCommand.Tick)), M(uc, nameof(UnitCommand.Interrupt)), M(uc, "ForceFinish"),
                     M(uc, nameof(UnitCommand.ForceFinishForTurnBased)), M(uc, "OnEnded"), M(uc, nameof(UnitCommand.OnRun)), M(uc, "StartInTbm"),
                     M(uc, "EndInTbm"), M(uc, "get_IsRunning"), M(uc, "get_ShouldUnitApproach"), M(uc, "get_IsIgnoreCooldown"),
                     M(uc, nameof(UnitCommand.IsSpellCombatAttack), typeof(UnitEntityData)), M0(uc, nameof(UnitCommand.IsSpellCombatAttack)),
                     M(uc, "get_TargetUnit"), M(uc, nameof(UnitCommand.SetIsActed)),
                     M(tc, nameof(TurnController.AddRunningCommand)), M(tc, nameof(TurnController.RemoveRunningCommand)),
                     M(tc, nameof(TurnController.IsRunningCommand)), M(tc, "get_IsActing"), M(tc, "get_IsEnding"), M(tc, nameof(TurnController.OnInterruptCommand)),
                 })
        {
            if (m == null) { Health.Failed("command observe: a base method is missing", new Exception("base")); continue; }
            BaseSet.Add(H(m));
        }
        Transparent.Add(H(M(typeof(object), "Equals", typeof(object), typeof(object))));

        P(runPrivate, nameof(RunPrefix), finalizer: nameof(RunFinalizer));
        P(queue, nameof(QueuePrefix), finalizer: nameof(QueueFinalizer));
        P(tickOnUnit, nameof(TickOnUnitPrefix), finalizer: nameof(TickOnUnitFinalizer));
        foreach (var m in mutators) P(m, nameof(MutatorPrefix), finalizer: nameof(MutatorFinalizer));
        P(M(typeof(UnitEntityData), nameof(UnitEntityData.ReplaceCommands)), nameof(ReplacePrefix));
        P(M(ctl, "TickCommand"), nameof(TickCommandPrefix), finalizer: nameof(TickCommandFinalizer));
        P(M(typeof(BaseUnitController), nameof(BaseUnitController.Tick)), nameof(ControllerTickPrefix), finalizer: nameof(ControllerTickFinalizer));

        // Leaves.
        void L(MethodBase m, string name, string postfix)
        {
            if (m == null) { Health.Failed("command observe: a leaf is missing", new Exception(name)); return; }
            LeafNames[H(m)] = name;
            P(m, postfix: postfix);
        }
        L(M0(uc, nameof(UnitCommand.IsUnitCloseEnough)), "close", nameof(CommandBoolPostfix));
        L(M(uc, "get_CannotStop"), "nostop", nameof(CommandBoolPostfix));
        foreach (var (getter, name) in new[]
                 {
                     ("get_ShouldBeInterrupted", "sbi"), ("get_IsInterruptible", "intr"), ("get_CanStart", "canstart"),
                     ("get_AwaitMovementFinish", "await"), ("get_DontWaitForHands", "dwfh"), ("get_IsOneFrameCommand", "oneframe"),
                     ("get_CanMoveAfterStart", "cmas"), ("get_ShouldTurnToTarget", "stt"), ("get_NeedEquipWeapons", "nequip"),
                 })
            foreach (var m in Virtual(getter)) L(m, name, nameof(CommandBoolPostfix));
        foreach (var m in Virtual("TryMergeInto")) L(m, "merge", nameof(MergePostfix));
        L(M(uc, nameof(UnitCommand.IsFullRoundAction)), "full", nameof(CommandBoolPostfix));
        L(M(typeof(UnitEntityView), nameof(UnitEntityView.IsMoving)), "moving", nameof(UnitBoolPostfix));
        L(M(typeof(UnitEntityView), "get_IsGetUp"), "getup", nameof(UnitBoolPostfix));
        foreach (var t in typeof(UnitMovementAgentBase).Assembly.GetTypes().Where(t => t.IsSubclassOf(typeof(UnitMovementAgentBase))))
        {
            var m = M(t, "get_IsReallyMoving");
            if (m != null && !m.IsAbstract) L(m, "rmoving", nameof(UnitBoolPostfix));
        }
        L(M(typeof(UnitEntityData), "get_AreHandsBusyWithAnimation"), "hands", nameof(UnitBoolPostfix));
        L(M(typeof(UnitEntityData), "get_IsDirectlyControllable"), "ctrl", nameof(UnitBoolPostfix));
        L(M(typeof(EntityDataBase), "get_IsInState"), "instate", nameof(UnitBoolPostfix));
        L(M(typeof(Kingmaker.Controllers.Combat.UnitCombatState), "get_CanActInCombat"), "caic", nameof(UnitBoolPostfix));
        L(M(typeof(Kingmaker.UnitLogic.UnitState), "get_CanAct"), "canact", nameof(UnitBoolPostfix));
        L(M(typeof(UnitHandEquipmentController), nameof(UnitHandEquipmentController.IsUpdateScheduledFor)), "equip", nameof(UnitArgPostfix));
        L(M(typeof(UnitProneController), nameof(UnitProneController.ShouldBeProne)), "shprone", nameof(UnitArgPostfix));
        L(M(uc, nameof(UnitCommand.CommandTargetUntargetable)), "untgt", nameof(UntargetablePostfix));
        L(M(typeof(UnitEntityData), nameof(UnitEntityData.IsEnemy), typeof(UnitEntityData)), "enemy", nameof(EnemyPostfix));
        L(M(typeof(UnitStealthExtension), nameof(UnitStealthExtension.InStealthFor)), "stealth", nameof(StealthPostfix));
        L(M(typeof(Kingmaker.UnitLogic.UnitHelper), nameof(Kingmaker.UnitLogic.UnitHelper.GetSaddledUnit)), "saddled", nameof(SaddledPostfix));
        L(M(ucs, "TryUseCustomHandler"), "custom", nameof(CustomPostfix));
        L(M(typeof(TargetWrapper), nameof(TargetWrapper.Equals), typeof(object)), "teq", nameof(TargetEqualsPostfix));

        // Class code.
        foreach (var name in new[] { "OnStart", "OnTick", "Init" })
            foreach (var m in Virtual(name))
                P(m, nameof(FxPrefix), finalizer: nameof(FxFinalizer));
        foreach (var m in Virtual("OnAction")) P(m, nameof(FxPrefix), finalizer: nameof(FxActionFinalizer));
        // An overridden OnRun (the base one is lifecycle code).
        foreach (var t in subclasses)
        {
            var o = M(t, nameof(UnitCommand.OnRun));
            if (o != null && !o.IsAbstract) P(o, nameof(FxPrefix), finalizer: nameof(FxFinalizer));
        }

        // UnitAttack's code (observer "attacks"): every method declared on it and its nested (compiler-generated)
        // types is lifecycle code, but for the leaves it answers and ConfigureAnimations; the getters between
        // InitAttacks and the attack mode too.
        {
            var ua = typeof(UnitAttack);
            // ConfigureAnimations (with TryToReorder and GetAnimationDuration) is an input: ua_atk has what it
            // decides, and its reads (GetSaddledUnit per attack, the animation manager) aren't walked.
            var leavesOf = new HashSet<string>
            {
                "TryMergeInto", "get_ShouldBeInterrupted", "get_IsInterruptible", "get_CanMoveAfterStart", "get_NeedEquipWeapons",
                "IsUnitInFiveFeetRange", ".ctor", ".cctor", "CreateAttackCommand",
                "ConfigureAnimations", "TryToReorder", "GetAnimationDuration",
            };
            var types = new List<Type> { ua };
            for (var k = 0; k < types.Count; k++) types.AddRange(types[k].GetNestedTypes(BindingFlags.Public | BindingFlags.NonPublic));
            var n0 = BaseSet.Count;
            foreach (var t in types)
                foreach (var m in t.GetMethods(all))
                    if (!m.IsAbstract && !(t == ua && leavesOf.Contains(m.Name))) BaseSet.Add(H(m));
            foreach (var m in new MethodBase[]
                     {
                         AccessTools.PropertyGetter(typeof(Kingmaker.Controllers.Combat.UnitCombatState), "IsFullAttackRestrictedBecauseOfMoveAction"),
                         AccessTools.PropertyGetter(typeof(Kingmaker.Controllers.Combat.UnitCombatState.TBMState), "EnabledFullAttack"),
                     })
            {
                if (m == null) { Health.Failed("command observe: an attack base method is missing", new Exception("attacks")); continue; }
                BaseSet.Add(H(m));
            }
            if (BaseSet.Count - n0 < 20) Health.Failed("command observe: UnitAttack's methods not found", new Exception((BaseSet.Count - n0).ToString()));
            L(M(ua, "IsUnitInFiveFeetRange"), "five", nameof(FivePostfix));
            L(M(tc, nameof(TurnController.GetEnabledFullAttack)), "efa", nameof(UnitArgPostfix));
            // Base code and a leaf: the attack mode inside it is recorded (efa), and its value (outside turn-based
            // combat it reads m_LastUsageOfMoveActionTime).
            L(AccessTools.PropertyGetter(typeof(Kingmaker.Controllers.Combat.UnitCombatState), "IsFullAttackRestrictedBecauseOfMoveAction"), "farm", nameof(UnitBoolPostfix));
            P(M(ua, "InitAttacks"), nameof(InitAttacksPrefix), finalizer: nameof(InitAttacksFinalizer));
            P(M(ua, "ConfigureAnimations"), nameof(ConfigureAnimationsPrefix));
            var tr = new HarmonyMethod(typeof(CommandObs), nameof(AttackTapTranspiler));
            foreach (var name in new[] { "OnTick", "UpdateTarget" })
            {
                var m = M(ua, name);
                if (m == null) { Health.Failed("command observe: UnitAttack." + name + " is missing", new Exception("attacks")); continue; }
                try { h.Patch(m, transpiler: tr); }
                catch (Exception e) { Health.Failed("patch UnitAttack." + name + " (taps)", e); }
            }
            foreach (var k in new[] { "OnTick:TapFinished", "OnTick:TapReleased", "OnTick:TapStyle", "UpdateTarget:TapEnemies", "UpdateTarget:TapSort" })
                if (!Tapped.TryGetValue(k, out var n) || n != 1) Health.Failed("command observe: tap " + k + " replaced " + (Tapped.TryGetValue(k, out var x) ? x : 0) + " calls, not 1", new Exception("attacks"));
            AttacksOn = true;
        }

        // The approach (observer "approach"): TickApproaching is base code (its command pushed for the taps and calls
        // inside it), an overridden OnTickApproaching class code; UnitMoveTo's and UnitAttackOfOpportunity's code is
        // base code but for the leaves they answer; the lifecycle's calls into the movement agent are recorded.
        {
            var tickApproaching = M(uc, nameof(UnitCommand.TickApproaching));
            var baseOta = M(uc, "OnTickApproaching");
            if (tickApproaching == null || baseOta == null) Health.Failed("command observe: TickApproaching is missing", new Exception("approach"));
            else { BaseSet.Add(H(tickApproaching)); BaseSet.Add(H(baseOta)); }
            P(tickApproaching, nameof(ApproachPrefix), finalizer: nameof(ApproachFinalizer));
            foreach (var t in subclasses)
            {
                var o = M(t, "OnTickApproaching");
                if (o != null && !o.IsAbstract) P(o, nameof(FxPrefix), finalizer: nameof(FxFinalizer));
            }
            var leavesOf = new Dictionary<Type, HashSet<string>>
            {
                [typeof(UnitMoveTo)] = new HashSet<string> { "get_AwaitMovementFinish", "get_DontWaitForHands", "get_AiCanInterrupt", "AddRiderCommand", ".ctor" },
                [typeof(UnitAttackOfOpportunity)] = new HashSet<string> { "get_ShouldBeInterrupted", "get_ShouldTurnToTarget", ".ctor" },
            };
            foreach (var kv in leavesOf)
            {
                var n0 = BaseSet.Count;
                foreach (var m in kv.Key.GetMethods(all))
                    if (!m.IsAbstract && !kv.Value.Contains(m.Name)) BaseSet.Add(H(m));
                if (BaseSet.Count - n0 < 5) Health.Failed("command observe: " + kv.Key.Name + "'s methods not found", new Exception((BaseSet.Count - n0).ToString()));
            }
            var tr = new HarmonyMethod(typeof(CommandObs), nameof(ApproachTapTranspiler));
            foreach (var m in new[] { tickApproaching, M(typeof(UnitMoveTo), "OnAction"), M(typeof(UnitAttackOfOpportunity), "Init"), M(typeof(UnitAttackOfOpportunity), "OnRun") })
            {
                if (m == null) { Health.Failed("command observe: an approach method is missing", new Exception("approach")); continue; }
                try { h.Patch(m, transpiler: tr); }
                catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name + " (taps)", e); }
            }
            foreach (var (k, want) in new[]
                     {
                         ("UnitCommand.TickApproaching:TapSaddledPart", 1), ("UnitCommand.TickApproaching:TapRiderPart", 1),
                         ("UnitCommand.TickApproaching:TapTactical", 1), ("UnitCommand.TickApproaching:TapWants", 1),
                         ("UnitCommand.TickApproaching:TapHasMovement", 2), ("UnitCommand.TickApproaching:TapCanMove", 2),
                         ("UnitCommand.TickApproaching:TapUcgu", 1), ("UnitCommand.TickApproaching:TapForcedPath", 1),
                         ("UnitCommand.TickApproaching:TapPathVisualizer", 1), ("UnitCommand.TickApproaching:TapCurrentPath", 1),
                         ("UnitCommand.TickApproaching:TapViewOr", 1),
                         ("UnitMoveTo.OnAction:TapPreventMovementFinish", 1), ("UnitAttackOfOpportunity.Init:TapThreatHand", 1),
                         ("UnitAttackOfOpportunity.OnRun:TapAnimationManager", 1), ("UnitAttackOfOpportunity.OnRun:TapIdle", 1),
                     })
                if (!Tapped.TryGetValue(k, out var n) || n != want)
                    Health.Failed("command observe: tap " + k + " replaced " + (Tapped.TryGetValue(k, out var x) ? x : 0) + " calls, not " + want, new Exception("approach"));
            P(M(typeof(UnitEntityView), nameof(UnitEntityView.MoveTo)), nameof(MoveToPrefix));
            P(M(typeof(UnitMovementAgent), nameof(UnitMovementAgent.FollowPrecomputedPath)), nameof(FollowPrefix));
            P(M(typeof(UnitEntityView), nameof(UnitEntityView.StopMoving)), nameof(StopPrefix));
            ApproachOn = true;
        }

        // Ends and the running list.
        P(M(uc, "OnEnded"), nameof(EndedPrefix), nameof(EndedPostfix));
        P(M(tc, nameof(TurnController.AddRunningCommand)), nameof(RunningPrefix), nameof(RunningPostfix));
        P(M(tc, nameof(TurnController.RemoveRunningCommand)), nameof(RunningPrefix), nameof(RunningPostfix));

        // The AI (observer "ai", CommandObsAi.cs).
        InstallAi(h, (m, prefix, postfix, finalizer) => P(m, prefix, postfix, finalizer));

        // The engagement controller (observer "engage", CommandObsEngage.cs).
        InstallEngage(h, (m, prefix, postfix, finalizer) => P(m, prefix, postfix, finalizer));

        // LastTarget and ManualTarget (observer "targets", CommandObsTarget.cs).
        InstallTargets(h, (m, prefix, postfix, finalizer) => P(m, prefix, postfix, finalizer));

        // The impatience controller (observer "impatience", CommandObsImpatience.cs).
        InstallImpatience(h, (m, prefix, postfix, finalizer) => P(m, prefix, postfix, finalizer));

        // The movement agent (observer "agent", CommandObsAgent.cs).
        InstallAgent(h, (m, prefix, postfix, finalizer) => P(m, prefix, postfix, finalizer));
    }
}
