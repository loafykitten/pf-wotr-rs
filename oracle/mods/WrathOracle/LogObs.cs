using System;
using System.Collections;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Runtime.CompilerServices;
using HarmonyLib;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.Localization;
using Kingmaker.RuleSystem;
using Kingmaker.UI.Models.Log;
using Kingmaker.UI.Models.Log.CombatLog_ThreadSystem;
using Kingmaker.UI.Models.Log.Events;
using UnityEngine;

namespace WrathOracle;

// Observer "combat_log" (docs/poc5/combat-log.md): the combat log as the game builds it, so a replay can make every
// message from its own rules and check it.
//  - the begin record's game_log: GameLogController's dispatch tables as its Start built them (the root rule types,
//    m_RootRulesTypes, sorted; each event type's threads, m_EventToThreads, in the dictionary's order, the threads
//    named by type and an ordinal), the events already queued (m_ReadyEvents) and open (m_EventsStack), and
//    GameLogContext's static fields (their m_Value, read without the getters, which log outside a scope), with the
//    scope's count. Some of those fields outlive every scope (Clear leaves SourceFact, Formula, Rations, RoundNumber
//    and RollChance), so a message can read a value from before the window.
//  - clog_q: each GameLogController.AddReadyEvent (a rule event when its rule completes, OnAfterEventDidTrigger; any
//    other event from its GameEventsHandler): the event's id (per window, in queue order), its type, its rule's id
//    (Rules.IdOf) or its own fields (units by id, rules by id, values; other objects by type), and, after the call,
//    m_IsEnabled and OverrideMessage's key; a non-rule event also has its caller chain.
//  - clog_tick: GameLogController.Tick's prefix, when anything is queued or open: the queued events' ids in order,
//    each one's IsEnabled as the tick will read it (GameLogRuleEvent's reads its rule's DisableBattleLog: the
//    DisableBattleLogSelf property and the units' DisableBattleLog properties, auto-properties), and the open stack's
//    size. The tick handles at most five events (MaxLogsInFrame); the rest stay queued.
//  - clog_msg: each message a thread adds (LogThreadBase.AddMessage, when the thread's list grew): the thread (type,
//    ordinal), the event being handled (its id; null outside a handler), the message's text, colour
//    (ColorUtility.ToHtmlStringRGBA), prefix icon and tooltip (class; header and description for a
//    TooltipTemplateSimple, read from the auto-properties' backing fields), and GameLogContext's fields at that point
//    (a diagnostic: the template inputs are the sim's to compute, never read from here).
// Every read is a field or an auto-property; nothing here calls game code that changes state. Records are written on
// the main thread inside turn-based windows (Sched.Obs).
internal static class LogObs
{
    internal static bool Installed;

    private static bool On => Installed && Sched.On;

    private const BindingFlags Inst = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic;

    private static readonly FieldInfo ReadyField = AccessTools.Field(typeof(GameLogController), "m_ReadyEvents");
    private static readonly FieldInfo StackField = AccessTools.Field(typeof(GameLogController), "m_EventsStack");
    private static readonly FieldInfo RootsField = AccessTools.Field(typeof(GameLogController), "m_RootRulesTypes");
    private static readonly FieldInfo ThreadsField = AccessTools.Field(typeof(GameLogController), "m_EventToThreads");
    private static readonly FieldInfo EnabledField = AccessTools.Field(typeof(GameLogEvent), "m_IsEnabled");
    private static readonly FieldInfo ThreadMessagesField = AccessTools.Field(typeof(LogThreadBase), "m_ThreadMessages");
    private static readonly FieldInfo TooltipField = AccessTools.Field(typeof(CombatLogMessage), "Tooltip");
    private static readonly FieldInfo ScopeFlagField = AccessTools.Field(typeof(GameLogContext), "ScopeFlag");
    private static readonly FieldInfo LocKeyField = AccessTools.Field(typeof(LocalizedString), "m_Key");

    // Per window: event ids (queue order), thread ordinals, the events of the current tick, the handlers running.
    private sealed class Box { public int Id; }
    private static ConditionalWeakTable<object, Box> _eventIds = new ConditionalWeakTable<object, Box>();
    private static ConditionalWeakTable<object, Box> _threadIds = new ConditionalWeakTable<object, Box>();
    private static int _nextEvent, _nextThread, _nextMsg;
    private static readonly List<object> TickEvents = new List<object>();
    private static readonly List<int> Handling = new List<int>();

    public static void ResetWindow()
    {
        _eventIds = new ConditionalWeakTable<object, Box>();
        _threadIds = new ConditionalWeakTable<object, Box>();
        _nextEvent = _nextThread = _nextMsg = 0;
        TickEvents.Clear();
        Handling.Clear();
    }

    private static GameLogController Controller => Kingmaker.Game.Instance?.GameLogController;

    // ---- names and values ----

    private static string TypeName(Type t)
    {
        if (t == null) return null;
        if (!t.IsGenericType) return t.Name;
        var n = t.Name;
        var tick = n.IndexOf('`');
        if (tick >= 0) n = n.Substring(0, tick);
        return n + "<" + string.Join(",", t.GetGenericArguments().Select(TypeName)) + ">";
    }

    private static int EventId(object e)
    {
        if (e == null) return 0;
        if (!_eventIds.TryGetValue(e, out var b))
        {
            b = new Box { Id = ++_nextEvent };
            _eventIds.Add(e, b);
        }
        return b.Id;
    }

    private static int? KnownEventId(object e) => e != null && _eventIds.TryGetValue(e, out var b) ? b.Id : (int?)null;

    private static int ThreadId(object th)
    {
        if (!_threadIds.TryGetValue(th, out var b))
        {
            b = new Box { Id = _nextThread++ };
            _threadIds.Add(th, b);
        }
        return b.Id;
    }

    private static string LocKey(LocalizedString s) => s == null ? null : (string)LocKeyField.GetValue(s);

    // A value as the records write it: units by id, rules by their trace id, facts by id and blueprint, localized
    // strings by key, primitives and enums as values, anything else by its type.
    private static void Val(Json j, object v)
    {
        switch (v)
        {
            case null: j.W.WriteNull(); return;
            case string s: j.W.WriteValue(s); return;
            case bool b: j.W.WriteValue(b); return;
            case int i: j.W.WriteValue(i); return;
            case long l: j.W.WriteValue(l); return;
            case float f: j.W.WriteValue(f); return;
            case double d: j.W.WriteValue(d); return;
            case Enum e: j.W.WriteValue(Convert.ToInt64(e)); return;
            case UnitEntityData u: j.W.WriteStartObject(); j.W.WritePropertyName("$unit"); j.W.WriteValue(u.UniqueId); j.W.WriteEndObject(); return;
            case RulebookEvent r: j.W.WriteStartObject(); j.W.WritePropertyName("$rule"); j.W.WriteValue(Rules.IdOf(r)); j.W.WritePropertyName("class"); j.W.WriteValue(r.GetType().Name); j.W.WriteEndObject(); return;
            case EntityFact f:
                j.W.WriteStartObject();
                j.W.WritePropertyName("$fact"); j.W.WriteValue(f.UniqueId);
                j.W.WritePropertyName("bp"); j.W.WriteValue(Snap.Guid(f.Blueprint));
                j.W.WriteEndObject();
                return;
            case LocalizedString ls: j.W.WriteStartObject(); j.W.WritePropertyName("$key"); j.W.WriteValue(LocKey(ls)); j.W.WriteEndObject(); return;
            case EntityDataBase ent: j.W.WriteStartObject(); j.W.WritePropertyName("$entity"); j.W.WriteValue(ent.UniqueId); j.W.WritePropertyName("class"); j.W.WriteValue(ent.GetType().Name); j.W.WriteEndObject(); return;
            default: j.W.WriteStartObject(); j.W.WritePropertyName("$type"); j.W.WriteValue(TypeName(v.GetType())); j.W.WriteEndObject(); return;
        }
    }

    // GameLogContext's static Property<T> fields, each one's m_Value (the getter logs outside a scope).
    private static readonly FieldInfo[] ContextFields = typeof(GameLogContext)
        .GetFields(BindingFlags.Static | BindingFlags.Public)
        .Where(f => f.FieldType.IsGenericType && f.FieldType.Name.StartsWith("Property", StringComparison.Ordinal))
        .ToArray();

    private static List<KeyValuePair<string, object>> ContextNow()
    {
        var l = new List<KeyValuePair<string, object>>();
        foreach (var f in ContextFields)
        {
            var boxed = f.GetValue(null);
            var inner = f.FieldType.GetField("m_Value", Inst);
            l.Add(new KeyValuePair<string, object>(f.Name, inner?.GetValue(boxed)));
        }
        return l;
    }

    private static int ScopeCount()
    {
        var flag = ScopeFlagField?.GetValue(null);
        return flag is Kingmaker.Utility.CountableFlag c ? c.Count : -1;
    }

    private static void WriteContext(Json j, List<KeyValuePair<string, object>> ctx)
    {
        j.W.WriteStartObject();
        foreach (var kv in ctx)
        {
            j.W.WritePropertyName(kv.Key);
            // Property<RuleRollD20>/<RuleRollD100>: the roll rule; the result is the rule's own record.
            Val(j, kv.Value);
        }
        j.W.WriteEndObject();
    }

    // An event's own fields (declared below GameLogEvent; GameLogRuleEvent's Rule is written as "rule").
    private static List<KeyValuePair<string, object>> EventFields(object e)
    {
        var l = new List<KeyValuePair<string, object>>();
        for (var t = e.GetType(); t != null && t != typeof(GameLogEvent) && t != typeof(GameLogRuleEvent); t = t.BaseType)
            foreach (var f in t.GetFields(Inst | BindingFlags.DeclaredOnly))
            {
                var v = f.GetValue(e);
                // GameLogEventProjectileHit: the projectile's attack roll (its m_AttackRoll field).
                if (v is Kingmaker.Controllers.Projectiles.Projectile p)
                    v = AccessTools.Field(typeof(Kingmaker.Controllers.Projectiles.Projectile), "m_AttackRoll")?.GetValue(p);
                l.Add(new KeyValuePair<string, object>(f.Name, v));
            }
        return l;
    }

    // ---- the begin record ----

    public static void WriteSetup(Json j)
    {
        j.Obj();
        var gc = Controller;
        j.Prop("controller", gc != null);
        if (gc != null)
        {
            var roots = ((IEnumerable)RootsField.GetValue(gc)).Cast<Type>().Select(TypeName).OrderBy(x => x, StringComparer.Ordinal).ToList();
            j.Arr("roots"); foreach (var r in roots) j.W.WriteValue(r); j.EndArr();
            j.Arr("threads");
            foreach (DictionaryEntry kv in (IDictionary)ThreadsField.GetValue(gc))
            {
                j.W.WriteStartObject();
                j.Prop("event", TypeName((Type)kv.Key));
                j.Arr("by");
                foreach (var th in (IEnumerable)kv.Value)
                {
                    j.W.WriteStartArray(); j.W.WriteValue(th.GetType().Name); j.W.WriteValue(ThreadId(th)); j.W.WriteEndArray();
                }
                j.EndArr();
                j.W.WriteEndObject();
            }
            j.EndArr();
            j.Arr("ready");
            foreach (var e in (IEnumerable)ReadyField.GetValue(gc)) WriteEvent(j, e, EventId(e));
            j.EndArr();
            j.Prop("stack", ((ICollection)StackField.GetValue(gc)).Count);
        }
        j.Prop("scope", ScopeCount());
        j.Key("ctx"); WriteContext(j, ContextNow());
        j.End();
    }

    private static void WriteEvent(Json j, object e, int id)
    {
        j.W.WriteStartObject();
        j.Prop("id", id);
        j.Prop("e", TypeName(e.GetType()));
        if (e is GameLogRuleEvent re) j.Prop("rule", Rules.IdOf(re.Rule));
        else
        {
            j.Key("f"); j.W.WriteStartObject();
            foreach (var kv in EventFields(e)) { j.W.WritePropertyName(kv.Key); Val(j, kv.Value); }
            j.W.WriteEndObject();
        }
        j.Prop("en", (bool)EnabledField.GetValue(e));
        j.Prop("ov", LocKey(((GameLogEvent)e).OverrideMessage));
        j.W.WriteEndObject();
    }

    // ---- GameLogController.AddReadyEvent ----

    public static void QueuedPostfix(GameLogEvent @event)
    {
        try
        {
            if (!On || @event == null) return;
            var id = EventId(@event);
            var type = TypeName(@event.GetType());
            var re = @event as GameLogRuleEvent;
            var rule = re != null ? Rules.IdOf(re.Rule) : (int?)null;
            var fields = re == null ? EventFields(@event) : null;
            var en = (bool)EnabledField.GetValue(@event);
            var ov = LocKey(@event.OverrideMessage);
            var origin = re == null ? Observe.Callers(10) : null;
            Sched.Obs("clog_q", j =>
            {
                j.Prop("id", id);
                j.Prop("e", type);
                j.Prop("rule", rule);
                if (fields != null)
                {
                    j.Key("f"); j.W.WriteStartObject();
                    foreach (var kv in fields) { j.W.WritePropertyName(kv.Key); Val(j, kv.Value); }
                    j.W.WriteEndObject();
                }
                j.Prop("en", en);
                j.Prop("ov", ov);
                if (origin != null) j.Prop("origin", origin);
            });
        }
        catch (Exception x) { Trace.HookFailed("observe clog_q", x); }
    }

    // ---- GameLogController.Tick ----

    public static void TickPrefix(GameLogController __instance)
    {
        TickEvents.Clear();
        Handling.Clear();
        try
        {
            if (!On) return;
            var ready = ((IEnumerable)ReadyField.GetValue(__instance)).Cast<object>().ToList();
            var stack = ((ICollection)StackField.GetValue(__instance)).Count;
            TickEvents.AddRange(ready);
            if (ready.Count == 0 && stack == 0) return;
            var ids = ready.Select(e => KnownEventId(e)).ToList();
            var en = ready.Select(e => ((GameLogEvent)e).IsEnabled).ToList();
            var scope = ScopeCount();
            Sched.Obs("clog_tick", j =>
            {
                j.Arr("q");
                for (var i = 0; i < ids.Count; i++)
                {
                    j.W.WriteStartArray();
                    if (ids[i].HasValue) j.W.WriteValue(ids[i].Value); else j.W.WriteNull();
                    j.W.WriteValue(en[i]);
                    j.W.WriteEndArray();
                }
                j.EndArr();
                j.Prop("stack", stack);
                j.Prop("scope", scope);
            });
        }
        catch (Exception x) { Trace.HookFailed("observe clog_tick", x); }
    }

    public static void TickFinalizer()
    {
        TickEvents.Clear();
        Handling.Clear();
    }

    // ---- the threads' handlers (which event a message comes from) ----

    public static void HandlePrefix(object[] __args)
    {
        try
        {
            if (!On) { Handling.Add(0); return; }
            var arg = __args != null && __args.Length > 0 ? __args[0] : null;
            var id = 0;
            foreach (var e in TickEvents)
            {
                if (ReferenceEquals(e, arg) || (e is GameLogRuleEvent re && ReferenceEquals(re.Rule, arg)))
                {
                    id = KnownEventId(e) ?? 0;
                    break;
                }
            }
            Handling.Add(id);
        }
        catch (Exception x) { Handling.Add(0); Trace.HookFailed("observe clog handler", x); }
    }

    public static void HandleFinalizer()
    {
        if (Handling.Count > 0) Handling.RemoveAt(Handling.Count - 1);
    }

    // ---- LogThreadBase.AddMessage ----

    private static int Count(object thread) => ThreadMessagesField.GetValue(thread) is ICollection c ? c.Count : -1;

    public static void AddPrefix(object __instance, out int __state)
    {
        __state = -1;
        try { if (On) __state = Count(__instance); }
        catch (Exception x) { Trace.HookFailed("observe clog_msg", x); }
    }

    private static string Backing(object o, string prop) =>
        o == null ? null : AccessTools.Field(o.GetType(), "<" + prop + ">k__BackingField")?.GetValue(o) as string;

    public static void AddPostfix(object __instance, CombatLogMessage newMessage, int __state)
    {
        try
        {
            if (!On || __state < 0 || newMessage == null) return;
            if (Count(__instance) <= __state) return;
            var n = _nextMsg++;
            var thread = __instance.GetType().Name;
            var ordinal = ThreadId(__instance);
            int? src = Handling.Count > 0 ? Handling[Handling.Count - 1] : (int?)null;
            if (src == 0) src = null;
            var inHandler = Handling.Count > 0;
            var text = newMessage.Message;
            var color = ColorUtility.ToHtmlStringRGBA(newMessage.Color);
            var icon = (int)newMessage.PrefixIcon;
            var tip = TooltipField.GetValue(newMessage);
            var tipClass = tip != null ? TypeName(tip.GetType()) : null;
            var tipHeader = Backing(tip, "Header");
            var tipDesc = Backing(tip, "Description");
            var ctx = ContextNow();
            var scope = ScopeCount();
            Sched.Obs("clog_msg", j =>
            {
                j.Prop("n", n);
                j.Prop("th", thread);
                j.Prop("ti", ordinal);
                j.Prop("src", src);
                j.Prop("in_handler", inHandler);
                j.Prop("text", text);
                j.Prop("color", color);
                j.Prop("icon", icon);
                if (tip == null) j.Null("tip");
                else
                {
                    j.Obj("tip");
                    j.Prop("cls", tipClass);
                    j.Prop("h", tipHeader);
                    j.Prop("d", tipDesc);
                    j.End();
                }
                j.Prop("scope", scope);
                j.Key("ctx"); WriteContext(j, ctx);
            });
        }
        catch (Exception x) { Trace.HookFailed("observe clog_msg", x); }
    }

    // ---- install ----

    public static void Install(Harmony h)
    {
        var failed = Health.FailedCount;
        foreach (var f in new[] { ReadyField, StackField, RootsField, ThreadsField, EnabledField, ThreadMessagesField, TooltipField, ScopeFlagField, LocKeyField })
            if (f == null) Health.Failed("combat log observe: a field is missing", new Exception("combat_log"));
        if (ContextFields.Length < 20) Health.Failed("combat log observe: GameLogContext's fields", new Exception(ContextFields.Length.ToString()));
        var gc = typeof(GameLogController);
        var add = AccessTools.Method(gc, "AddReadyEvent", new[] { typeof(GameLogEvent) });
        var tick = AccessTools.Method(gc, "Kingmaker.Controllers.IController.Tick") ?? gc.GetInterfaceMap(typeof(Kingmaker.Controllers.IController)).TargetMethods.FirstOrDefault(m => m.Name.EndsWith("Tick", StringComparison.Ordinal));
        var addMsg = AccessTools.Method(typeof(LogThreadBase), "AddMessage", new[] { typeof(CombatLogMessage) });
        if (add == null || tick == null || addMsg == null)
        {
            Health.Failed("combat log observe: GameLogController.AddReadyEvent/Tick or LogThreadBase.AddMessage missing", new Exception("combat_log"));
            return;
        }
        try
        {
            h.Patch(add, postfix: new HarmonyMethod(typeof(LogObs), nameof(QueuedPostfix)));
            h.Patch(tick, prefix: new HarmonyMethod(typeof(LogObs), nameof(TickPrefix)), finalizer: new HarmonyMethod(typeof(LogObs), nameof(TickFinalizer)));
            h.Patch(addMsg, prefix: new HarmonyMethod(typeof(LogObs), nameof(AddPrefix)), postfix: new HarmonyMethod(typeof(LogObs), nameof(AddPostfix)));
        }
        catch (Exception e) { Health.Failed("patch the combat log (clog_q, clog_tick, clog_msg)", e); return; }
        // Every thread's HandleEvent overloads (concrete methods of the LogThreadBase subclasses).
        var handlers = 0;
        foreach (var t in typeof(LogThreadBase).Assembly.GetTypes().Where(t => typeof(LogThreadBase).IsAssignableFrom(t) && !t.ContainsGenericParameters))
            foreach (var m in t.GetMethods(Inst | BindingFlags.DeclaredOnly))
            {
                if (m.Name != "HandleEvent" || m.IsAbstract || m.IsGenericMethodDefinition || m.GetParameters().Length != 1) continue;
                try
                {
                    h.Patch(m, prefix: new HarmonyMethod(typeof(LogObs), nameof(HandlePrefix)), finalizer: new HarmonyMethod(typeof(LogObs), nameof(HandleFinalizer)));
                    handlers++;
                }
                catch (Exception e) { Health.Failed("patch " + t.Name + ".HandleEvent", e); }
            }
        if (handlers < 40) Health.Failed("combat log observe: only " + handlers + " HandleEvent methods", new Exception("combat_log"));
        Installed = Health.FailedCount == failed;
    }
}
