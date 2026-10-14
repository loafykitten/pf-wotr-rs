using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using HarmonyLib;
using Kingmaker.Blueprints;
using Kingmaker.ElementsSystem;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.PubSubSystem;
using Kingmaker.RuleSystem;
using Kingmaker.UnitLogic.Mechanics;

namespace WrathOracle;

// Rules trace (Wrath-Rust PLAN, "The oracle": nested traces with parent IDs, inputs, modifier
// breakdowns and results, per-handler execution and state mutations).
//
// Observation points, in the game's order (Rulebook.TriggerEvent / TriggerEventInternal):
//   PushEvent (postfix)          "rule"      id, parent, initiator, target, reason, full field snapshot
//   mod Before handlers          "h"/"h_end" phase mod_before (IBeforeRulebookEventTriggerHandler<T>)
//   AboutTo hook (ours)          "about"     diff of the rule's fields since the last record
//   AboutTo subscribers          "h"/"h_end" phase about: global, target, initiator lists, in order
//   OnTrigger (prefix/postfix)   "trig"/"trig_end" diff; the rule's own computation (nested rules inside)
//   mod After handlers           "h"/"h_end" phase mod_after
//   DidTrigger subscribers       "h"/"h_end" phase did
//   DidTrigger hook (ours)       "did"       diff
//   OnDidTrigger (postfix)       "rule_end"  diff after temporary modifiers were removed, the bonuses
//   PopEvent (postfix)           closes the rule (no record; the structure check uses it)
// "h" records carry the handler's identity (class, component, owner blueprint, fact, owner
// unit); "h_end" carries the diff of the rule's fields across the handler. Mutations (stat
// modifiers, facts, damage, narrative state) are separate "mut" records attributed to the
// current rule and handler (Mutations.cs). Handler exceptions are recorded ("h_exc").
internal static class Rules
{
    private sealed class RuleState
    {
        public int Id, Parent, Depth;
        public List<KeyValuePair<string, string>> Last;
        public byte Seen; // 1 rule, 2 about, 4 trig, 8 trig_end, 16 did, 32 end, 64 pop
        public string Class;
        public bool Excused; // an exception inside the rule explains missing parts
        public UnitEntityData A, B; // initiator and target, for HP changes across the rule
        public int DamageA, DamageB;
        public int Draws; // draws made while this rule was the innermost open one
    }

    private sealed class HandlerState
    {
        public int Id, Rule;
        public RulebookEvent Evt;
        public List<KeyValuePair<string, string>> Before;
    }

    private static readonly Dictionary<RulebookEvent, RuleState> Open = new Dictionary<RulebookEvent, RuleState>(new RefEq<RulebookEvent>());
    private static readonly List<RuleState> RuleStack = new List<RuleState>();
    private static readonly List<HandlerState> HandlerStack = new List<HandlerState>();
    private static readonly List<RuleState> Closed = new List<RuleState>();
    private static int _nextRule, _nextHandler;
    public static long RulesSeen, HandlersSeen;

    // Fault injection (mutation tests): make our about-hook throw, or silently skip, at rule N.
    public static int FaultThrowAboutAt = -1, FaultDropAboutAt = -1;

    public static int CurrentRuleId => RuleStack.Count > 0 ? RuleStack[RuleStack.Count - 1].Id : 0;
    public static int OpenRules => RuleStack.Count;
    // Context of a draw made now: the innermost open rule's class, initiator, target and the
    // draw's ordinal within the rule (null outside rules). Units created during the window have
    // run-specific ids, so they are named by blueprint.
    public static string DrawContext()
    {
        if (RuleStack.Count == 0) return null;
        var s = RuleStack[RuleStack.Count - 1];
        return $"{s.Class}|{StableId(s.A)}|{StableId(s.B)}|{s.Draws++}";
    }

    private static string StableId(UnitEntityData u)
    {
        if (u == null) return "-";
        var id = u.UniqueId;
        return id.Length > 8 ? "new:" + u.Blueprint?.AssetGuid : id;
    }

    public static int CurrentHandlerId => HandlerStack.Count > 0 ? HandlerStack[HandlerStack.Count - 1].Id : 0;

    public static void Reset()
    {
        Open.Clear();
        AllIds.Clear();
        RuleStack.Clear();
        HandlerStack.Clear();
        Closed.Clear();
        _nextRule = 0;
        _nextHandler = 0;
        RulesSeen = HandlersSeen = 0;
    }

    // Ids of every rule seen in the window (closed ones too), for references between rules.
    private static readonly Dictionary<RulebookEvent, int> AllIds = new Dictionary<RulebookEvent, int>(new RefEq<RulebookEvent>());

    public static int IdOf(RulebookEvent r) => r == null ? 0 : Open.TryGetValue(r, out var s) ? s.Id : AllIds.TryGetValue(r, out var id) ? id : 0;

    private static readonly HashSet<string> SkipFields = new HashSet<string> { "Initiator", "Reason" };

    private static List<KeyValuePair<string, string>> Snapshot(RulebookEvent r) => Snap.FieldPairs(r, 1, SkipFields);

    private static void WriteDiff(Json j, RuleState s, RulebookEvent r, string key = "changes")
    {
        var now = Snapshot(r);
        j.Arr(key);
        var last = s.Last;
        var map = new Dictionary<string, string>();
        if (last != null) foreach (var kv in last) map[kv.Key] = kv.Value;
        foreach (var kv in now)
        {
            if (map.TryGetValue(kv.Key, out var old) && old == kv.Value) continue;
            j.W.WriteStartArray();
            j.W.WriteValue(kv.Key);
            j.W.WriteRawValue(kv.Value);
            j.W.WriteEndArray();
        }
        j.EndArr();
        s.Last = now;
    }

    private static void Diff(Json j, List<KeyValuePair<string, string>> before, List<KeyValuePair<string, string>> after)
    {
        var map = new Dictionary<string, string>();
        foreach (var kv in before) map[kv.Key] = kv.Value;
        j.Arr("changes");
        foreach (var kv in after)
        {
            if (map.TryGetValue(kv.Key, out var old) && old == kv.Value) continue;
            j.W.WriteStartArray();
            j.W.WriteValue(kv.Key);
            j.W.WriteRawValue(kv.Value);
            j.W.WriteEndArray();
        }
        j.EndArr();
    }

    // ---- push / pop ----

    public static void OnPush(RulebookEvent evt, RulebookEventContext context = null)
    {
        if (!Trace.Active || evt == null || !Trace.OnMainThread) return;
        try
        {
            var parent = RuleStack.Count > 0 ? RuleStack[RuleStack.Count - 1] : null;
            var s = new RuleState { Id = ++_nextRule, Parent = parent?.Id ?? 0, Depth = RuleStack.Count, Class = evt.GetType().FullName };
            Open[evt] = s;
            AllIds[evt] = s.Id;
            s.A = evt.Initiator;
            s.DamageA = s.A?.Descriptor?.Damage ?? 0;
            RuleStack.Add(s);
            RulesSeen++;
            s.Seen |= 1;
            Trace.Emit("rule", j =>
            {
                j.Prop("id", s.Id);
                j.Prop("parent", s.Parent);
                // The rule's parent in the game's own context: differs from the execution parent
                // when TriggerEventInRestoredContext swapped in a saved context.
                j.Prop("context_parent", context?.PreviousEvent != null ? IdOf(context.PreviousEvent) : 0);
                j.Prop("depth", s.Depth);
                j.Prop("class", s.Class);
                j.Prop("handler", CurrentHandlerId);
                j.Key("initiator"); Snap.UnitRef(j, evt.Initiator);
                j.Key("target");
                var getTarget = evt.GetType().GetMethod("GetRuleTarget", BindingFlags.Instance | BindingFlags.Public);
                if (getTarget != null && Purity.IsPure(getTarget))
                {
                    s.B = evt.GetRuleTarget();
                    s.DamageB = s.B?.Descriptor?.Damage ?? 0;
                    Snap.UnitRef(j, s.B);
                }
                else j.W.WriteValue("$impure");
                j.Key("reason_at_push"); Reason(j, evt.Reason); // usually null: AssignSource runs after the push
                // A root rule's call site (who triggered it: a controller, a command, the AI, the UI).
                if (s.Depth == 0) j.Prop("origin", Origin());
                var snap = Snapshot(evt);
                s.Last = snap;
                j.Obj("fields");
                foreach (var kv in snap)
                {
                    j.W.WritePropertyName(kv.Key);
                    j.W.WriteRawValue(kv.Value);
                }
                j.End();
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("rule push", e);
        }
    }

    // The game methods below the rulebook's own, innermost first (FullName.Method, joined by " < ":
    // compiler-generated names hold '<' and '|' but no spaces); the oracle's and Harmony's frames
    // and Rulebook/RulebookEventContext are left out.
    private static string Origin()
    {
        try
        {
            var frames = new System.Diagnostics.StackTrace(1, false).GetFrames();
            if (frames == null) return "?";
            return string.Join(" < ", frames.Select(Rng.FrameMethod)
                .Where(m => m != null && m.DeclaringType != null && !m.Name.StartsWith("DMD<", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("WrathOracle", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("HarmonyLib", StringComparison.Ordinal) &&
                            m.DeclaringType != typeof(Rulebook) && m.DeclaringType != typeof(RulebookEventContext))
                .Take(24).Select(m => m.DeclaringType.FullName + "." + m.Name));
        }
        catch (Exception) { return "?"; }
    }

    public static void OnPop(RulebookEvent evt)
    {
        if (evt == null || !Open.TryGetValue(evt, out var s)) return;
        s.Seen |= 64;
        Open.Remove(evt);
        // A handler of this rule still on the stack means a missing h_end.
        foreach (var h in HandlerStack.Where(h => ReferenceEquals(h.Evt, evt)).ToList())
        {
            Trace.Structural($"handler {h.Id} of rule {s.Id} never ended");
            HandlerStack.Remove(h);
        }
        // Pop everything above it too (a rule that threw out of its nested rules).
        var idx = RuleStack.LastIndexOf(s);
        if (idx >= 0)
        {
            for (var i = RuleStack.Count - 1; i > idx; i--)
            {
                Trace.Structural($"rule {RuleStack[i].Id} ({RuleStack[i].Class}) still open when its parent {s.Id} popped");
                RuleStack.RemoveAt(i);
            }
            RuleStack.RemoveAt(idx);
        }
        Closed.Add(s);
        CheckRule(s);
    }

    private static void CheckRule(RuleState s)
    {
        // Every rule that ran to completion must have all of these (an exception inside the
        // rule is recorded as h_exc / trig_exc and excuses the missing parts).
        const byte all = 1 | 2 | 4 | 8 | 16 | 32 | 64;
        if ((s.Seen & all) != all && !s.Excused)
        {
            var missing = new List<string>();
            if ((s.Seen & 2) == 0) missing.Add("about");
            if ((s.Seen & 4) == 0) missing.Add("trig");
            if ((s.Seen & 8) == 0) missing.Add("trig_end");
            if ((s.Seen & 16) == 0) missing.Add("did");
            if ((s.Seen & 32) == 0) missing.Add("rule_end");
            Trace.Structural($"rule {s.Id} ({s.Class}) missing {string.Join(",", missing)}");
        }
    }

    public static void CheckStructureAtEnd()
    {
        foreach (var s in RuleStack) Trace.Structural($"rule {s.Id} ({s.Class}) still open at the end of the window");
        foreach (var h in HandlerStack) Trace.Structural($"handler {h.Id} (rule {h.Rule}) still open at the end of the window");
        RuleStack.Clear();
        Open.Clear();
        HandlerStack.Clear();
    }

    private static void Reason(Json j, RuleReason reason)
    {
        if (reason == null) { j.W.WriteNull(); return; }
        j.W.WriteStartObject();
        try
        {
            j.Key("fact"); Snap.FactRef(j, reason.Fact);
            j.Prop("ability", Snap.SafeGuid(() => reason.Ability?.Blueprint));
            j.Prop("item", reason.Item?.UniqueId);
            j.Key("caster"); Snap.UnitRef(j, reason.Caster);
            j.Prop("context_blueprint", Snap.SafeGuid(() => reason.Context?.AssociatedBlueprint));
            j.Key("rule");
            if (reason.Rule == null) j.W.WriteNull(); else j.W.WriteValue(IdOf(reason.Rule));
        }
        catch (Exception e)
        {
            j.Prop("$failed", e.GetType().Name + ": " + e.Message);
        }
        j.W.WriteEndObject();
    }

    // ---- hooks (the game's IRulebookEvent*Hook interfaces) ----

    internal sealed class HookSubscriber : IRulebookEventAboutToTriggerHook, IRulebookEventDidTriggerHook
    {
        public void OnBeforeEventAboutToTrigger(RulebookEvent rule)
        {
            if (!Trace.Active || !Open.TryGetValue(rule, out var s)) return;
            if (s.Id == FaultDropAboutAt) return; // injected silent fault
            try
            {
                if (s.Id == FaultThrowAboutAt) throw new InvalidOperationException("injected fault in the about-hook");
                s.Seen |= 2;
                Trace.Emit("about", j =>
                {
                    j.Prop("id", s.Id);
                    j.Key("reason"); Reason(j, rule.Reason); // as Rulebook.AssignSource set it
                    WriteDiff(j, s, rule);
                });
            }
            catch (Exception e)
            {
                Trace.HookFailed("about", e);
            }
        }

        public void OnAfterEventDidTrigger(RulebookEvent rule)
        {
            if (!Trace.Active || !Open.TryGetValue(rule, out var s)) return;
            try
            {
                s.Seen |= 16;
                Trace.Emit("did", j => { j.Prop("id", s.Id); WriteDiff(j, s, rule); });
            }
            catch (Exception e)
            {
                Trace.HookFailed("did", e);
            }
        }
    }

    public static void OnDidTriggerPostfix(RulebookEvent __instance)
    {
        var evt = __instance;
        if (!Trace.Active || !Open.TryGetValue(evt, out var s)) return;
        try
        {
            s.Seen |= 32;
            // HP changes across the rule (the Damage setters are small enough that callers
            // may inline them, so they are observed here rather than hooked).
            foreach (var (u, before) in new[] { (s.A, s.DamageA), (s.B == s.A ? null : s.B, s.DamageB) })
            {
                if (u == null) continue;
                var now = u.Descriptor?.Damage ?? before;
                if (now == before) continue;
                Trace.Emit("mut", j => { j.Prop("what", "damage"); j.Prop("rule", s.Id); j.Prop("h", CurrentHandlerId); j.Prop("unit", u.UniqueId); j.Prop("old", before); j.Prop("new", now); });
                // Attributed to the innermost rule: enclosing rules take it as their new baseline.
                foreach (var o in RuleStack)
                {
                    if (o.A == u) o.DamageA = now;
                    if (o.B == u) o.DamageB = now;
                }
            }
            Trace.Emit("rule_end", j =>
            {
                j.Prop("id", s.Id);
                WriteDiff(j, s, evt);
                j.Arr("bonuses");
                foreach (var b in evt.AllBonuses)
                {
                    j.W.WriteStartObject();
                    j.Prop("value", b.Value);
                    j.Prop("descriptor", (int)b.Descriptor);
                    j.Prop("type", (int)b.Type);
                    j.Prop("stat", (int)b.Stat);
                    j.Prop("dice", b.DiceFormula);
                    j.Key("fact"); Snap.FactRef(j, b.Fact);
                    j.W.WriteEndObject();
                }
                j.EndArr();
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("rule_end", e);
        }
    }

    // ---- OnTrigger of each concrete rule class ----

    public static void TriggerPrefix(RulebookEvent __instance)
    {
        if (!Trace.Active || !Open.TryGetValue(__instance, out var s)) return;
        try
        {
            s.Seen |= 4;
            Trace.Emit("trig", j => { j.Prop("id", s.Id); WriteDiff(j, s, __instance); });
        }
        catch (Exception e)
        {
            Trace.HookFailed("trig", e);
        }
    }

    public static void TriggerPostfix(RulebookEvent __instance)
    {
        if (!Trace.Active || !Open.TryGetValue(__instance, out var s)) return;
        try
        {
            s.Seen |= 8;
            Trace.Emit("trig_end", j => { j.Prop("id", s.Id); WriteDiff(j, s, __instance); });
        }
        catch (Exception e)
        {
            Trace.HookFailed("trig_end", e);
        }
    }

    public static Exception TriggerFinalizer(RulebookEvent __instance, Exception __exception)
    {
        if (__exception != null && Trace.Active && Open.TryGetValue(__instance, out var s))
        {
            s.Excused = true;
            Trace.Emit("trig_exc", j => { j.Prop("id", s.Id); j.Prop("exception", __exception.GetType().FullName + ": " + __exception.Message); });
        }
        return __exception;
    }

    // ---- handlers ----

    private static readonly Dictionary<MethodBase, string> PhaseOf = new Dictionary<MethodBase, string>();

    public static void HandlerPrefix(object __instance, object __0, MethodBase __originalMethod, out object __state)
    {
        __state = null;
        if (!Trace.Active || !(__0 is RulebookEvent evt) || !Trace.OnMainThread) return;
        try
        {
            var rid = IdOf(evt);
            var h = new HandlerState { Id = ++_nextHandler, Rule = rid, Evt = evt, Before = Snapshot(evt) };
            HandlersSeen++;
            HandlerStack.Add(h);
            __state = h;
            PhaseOf.TryGetValue(__originalMethod, out var phase);
            Trace.Emit("h", j =>
            {
                j.Prop("id", h.Id);
                j.Prop("rule", rid);
                j.Prop("phase", phase);
                j.Prop("class", __instance?.GetType().FullName);
                Identity(j, __instance);
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("handler prefix", e);
        }
    }

    public static void HandlerPostfix(object __state)
    {
        if (!(__state is HandlerState h)) return;
        try
        {
            PopHandler(h);
            if (!Trace.Active) return;
            Trace.Emit("h_end", j =>
            {
                j.Prop("id", h.Id);
                j.Prop("rule", h.Rule);
                Diff(j, h.Before, Snapshot(h.Evt));
            });
            // Keep the rule's running snapshot in step, so "did"/"rule_end" diffs don't
            // repeat what handlers already reported.
            if (Open.TryGetValue(h.Evt, out var s)) s.Last = Snapshot(h.Evt);
        }
        catch (Exception e)
        {
            Trace.HookFailed("handler postfix", e);
        }
    }

    public static Exception HandlerFinalizer(object __state, Exception __exception)
    {
        if (__exception != null && __state is HandlerState h)
        {
            PopHandler(h);
            if (Trace.Active)
            {
                if (Open.TryGetValue(h.Evt, out var s)) s.Excused = true;
                Trace.Emit("h_exc", j => { j.Prop("id", h.Id); j.Prop("rule", h.Rule); j.Prop("exception", __exception.GetType().FullName + ": " + __exception.Message); });
            }
        }
        return __exception;
    }

    private static void PopHandler(HandlerState h)
    {
        var i = HandlerStack.LastIndexOf(h);
        if (i < 0) return;
        for (var k = HandlerStack.Count - 1; k >= i; k--) HandlerStack.RemoveAt(k);
    }

    // Who ran: for fact components, the component, its blueprint and the fact/owner of the
    // runtime currently in context (EntityFactComponentDelegate<,>.ComponentEventContext).
    private static readonly Dictionary<Type, Func<object>> RuntimeGetters = new Dictionary<Type, Func<object>>();

    private static void Identity(Json j, object handler)
    {
        if (handler is BlueprintComponent bc)
        {
            j.Prop("component", bc.name);
            j.Prop("blueprint", Snap.SafeGuid(() => bc.OwnerBlueprint));
            var rt = CurrentRuntime(handler.GetType()) as EntityFactComponent;
            if (rt != null)
            {
                j.Key("fact"); Snap.FactRef(j, rt.Fact);
                j.Key("owner"); Snap.Value(j, rt.Owner, Snap.Mode.Fields, 3);
            }
        }
        else if (handler is EntityFactComponent efc)
        {
            j.Key("fact"); Snap.FactRef(j, efc.Fact);
            j.Key("owner"); Snap.Value(j, efc.Owner, Snap.Mode.Fields, 3);
        }
    }

    private static object CurrentRuntime(Type componentType)
    {
        if (!RuntimeGetters.TryGetValue(componentType, out var get))
        {
            get = null;
            for (var t = componentType; t != null; t = t.BaseType)
            {
                if (t.IsGenericType && t.GetGenericTypeDefinition() == typeof(EntityFactComponentDelegate<,>))
                {
                    var ctx = typeof(EntityFactComponentDelegate<,>).GetNestedType("ComponentEventContext", BindingFlags.NonPublic | BindingFlags.Public)
                        ?.MakeGenericType(t.GetGenericArguments());
                    var current = ctx == null ? null : typeof(ContextData<>).MakeGenericType(ctx).GetProperty("Current", BindingFlags.Static | BindingFlags.Public);
                    var field = ctx?.GetField("m_Runtime", BindingFlags.Instance | BindingFlags.NonPublic);
                    if (current != null && field != null)
                        get = () => { var c = current.GetValue(null, null); return c == null ? null : field.GetValue(c); };
                    break;
                }
            }
            RuntimeGetters[componentType] = get;
        }
        try { return get?.Invoke(); } catch (Exception) { return null; }
    }

    // ---- installation ----

    public static int HandlersPatched, HandlersSkippedGeneric, TriggersPatched, TriggersSkipped;
    public static readonly List<string> SkippedHandlers = new List<string>();

    public static void Install(Harmony h)
    {
        EventBus.Subscribe(new HookSubscriber());
        h.Patch(AccessTools.Method(typeof(RulebookEventContext), nameof(RulebookEventContext.PushEvent)),
            postfix: new HarmonyMethod(typeof(Rules), nameof(PushPostfix)));
        h.Patch(AccessTools.Method(typeof(RulebookEventContext), nameof(RulebookEventContext.PopEvent)),
            postfix: new HarmonyMethod(typeof(Rules), nameof(PopPostfix)));
        h.Patch(AccessTools.Method(typeof(RulebookEvent), nameof(RulebookEvent.OnDidTrigger)),
            postfix: new HarmonyMethod(typeof(Rules), nameof(OnDidTriggerPostfix)));

        var asm = typeof(RulebookEvent).Assembly;
        Type[] types;
        try { types = asm.GetTypes(); } catch (ReflectionTypeLoadException e) { types = e.Types.Where(t => t != null).ToArray(); }

        // OnTrigger of every rule class that declares one.
        var tp = new HarmonyMethod(typeof(Rules), nameof(TriggerPrefix));
        var tq = new HarmonyMethod(typeof(Rules), nameof(TriggerPostfix));
        var tf = new HarmonyMethod(typeof(Rules), nameof(TriggerFinalizer));
        foreach (var t in types)
        {
            if (!typeof(RulebookEvent).IsAssignableFrom(t)) continue;
            var m = t.GetMethod("OnTrigger", BindingFlags.Instance | BindingFlags.Public | BindingFlags.DeclaredOnly, null, new[] { typeof(RulebookEventContext) }, null);
            if (m == null || m.IsAbstract) continue;
            if (t.ContainsGenericParameters) { TriggersSkipped++; continue; }
            try
            {
                h.Patch(m, prefix: tp, postfix: tq, finalizer: tf);
                TriggersPatched++;
            }
            catch (Exception e)
            {
                TriggersSkipped++;
                Health.Failed("patch OnTrigger " + t.FullName, e);
            }
        }

        // Every concrete handler method of IRulebookHandler<T> (about/did) and of the mod-facing
        // IBefore/IAfterRulebookEventTriggerHandler<T>.
        var hp = new HarmonyMethod(typeof(Rules), nameof(HandlerPrefix));
        var hq = new HarmonyMethod(typeof(Rules), nameof(HandlerPostfix));
        var hf = new HarmonyMethod(typeof(Rules), nameof(HandlerFinalizer));
        var targets = new HashSet<MethodInfo>();
        var assemblies = AppDomain.CurrentDomain.GetAssemblies().Where(a =>
        {
            if (a.IsDynamic) return false;
            string loc;
            try { loc = a.Location ?? ""; } catch (Exception) { return false; }
            var n = a.GetName().Name;
            return n == "Assembly-CSharp" || loc.Contains("/Mods/") && n != "WrathOracle";
        });
        foreach (var a in assemblies)
        {
            Type[] ts;
            try { ts = a.GetTypes(); } catch (ReflectionTypeLoadException e) { ts = e.Types.Where(t => t != null).ToArray(); }
            catch (Exception) { continue; }
            foreach (var t in ts)
            {
                if (t.IsInterface || t.IsAbstract && t.IsSealed) continue;
                Type[] ifaces;
                try { ifaces = t.GetInterfaces(); } catch (Exception) { continue; }
                foreach (var i in ifaces)
                {
                    if (!i.IsGenericType) continue;
                    var def = i.GetGenericTypeDefinition();
                    string phaseAbout, phaseDid;
                    if (def == typeof(IRulebookHandler<>)) { phaseAbout = "about"; phaseDid = "did"; }
                    else if (def == typeof(IBeforeRulebookEventTriggerHandler<>)) { phaseAbout = "mod_before"; phaseDid = null; }
                    else if (def == typeof(IAfterRulebookEventTriggerHandler<>)) { phaseAbout = "mod_after"; phaseDid = null; }
                    else continue;
                    if (t.IsAbstract || t.ContainsGenericParameters) continue; // the concrete subclasses map to the same methods
                    InterfaceMapping map;
                    try { map = t.GetInterfaceMap(i); } catch (Exception) { continue; }
                    foreach (var m in map.TargetMethods)
                    {
                        if (m == null || m.IsAbstract) continue;
                        var phase = m.Name.EndsWith("OnEventDidTrigger") ? phaseDid : phaseAbout;
                        if (phase == null) phase = phaseAbout;
                        if (m.DeclaringType == null || m.DeclaringType.IsGenericType || m.DeclaringType.ContainsGenericParameters || m.IsGenericMethod)
                        {
                            HandlersSkippedGeneric++;
                            if (SkippedHandlers.Count < 500) SkippedHandlers.Add(m.DeclaringType?.FullName + "." + m.Name);
                            continue;
                        }
                        // Normalise to the declaring type's own MethodInfo (an inherited method
                        // reflected through a subclass can't be patched, and dedupes badly).
                        var declared = m.DeclaringType.GetMethod(m.Name, BindingFlags.DeclaredOnly | BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic,
                            null, m.GetParameters().Select(p => p.ParameterType).ToArray(), null) ?? m;
                        if (targets.Add(declared)) PhaseOf[declared] = phase;
                    }
                }
            }
        }
        foreach (var m in targets)
        {
            try
            {
                h.Patch(m, prefix: hp, postfix: hq, finalizer: hf);
                HandlersPatched++;
            }
            catch (Exception e)
            {
                Health.Failed("patch handler " + m.DeclaringType?.FullName + "." + m.Name, e);
            }
        }
    }

    public static void PushPostfix(RulebookEventContext __instance, RulebookEvent evt) => OnPush(evt, __instance);

    public static void PopPostfix(RulebookEvent evt) => OnPop(evt);
}

internal sealed class RefEq<T> : IEqualityComparer<T> where T : class
{
    public bool Equals(T a, T b) => ReferenceEquals(a, b);
    public int GetHashCode(T o) => System.Runtime.CompilerServices.RuntimeHelpers.GetHashCode(o);
}
