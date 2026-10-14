using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using HarmonyLib;
using Kingmaker;

namespace WrathOracle;

// Which code allocates each game unique id (Player.GetNewUniqueId) - Wrath-Rust POC-5, for
// reproducing the ids that units created at load (placeholders' BecomeUnit) give their facts.
// Plan steps (fidelity plans; the hook is installed with the fidelity instrumentation):
//   idtrace:on            start recording (e.g. before a reload)
//   idtrace:write:LABEL   write ids-LABEL.json (id, then the calling methods, innermost first)
//                         and stop recording
internal static class IdTrace
{
    private static bool _on;
    private static readonly List<KeyValuePair<string, string[]>> Ids = new List<KeyValuePair<string, string[]>>();

    public static void Postfix(string __result)
    {
        if (!_on) return;
        try
        {
            var st = new StackTrace(1, false);
            var n = Math.Min(st.FrameCount, 24);
            var frames = new string[n];
            for (var i = 0; i < n; i++)
            {
                var m = Rng.FrameMethod(st.GetFrame(i));
                frames[i] = m == null ? "?" : (m.DeclaringType?.FullName ?? "?") + "." + m.Name;
            }
            Ids.Add(new KeyValuePair<string, string[]>(__result, frames));
        }
        catch (Exception e)
        {
            Health.Failed("idtrace", e);
        }
    }

    // Plan end or abort: stop recording and drop what was not written.
    public static void Stop()
    {
        _on = false;
        Ids.Clear();
    }

    public static void Install(Harmony h)
    {
        try
        {
            h.Patch(AccessTools.Method(typeof(Player), nameof(Player.GetNewUniqueId)), postfix: new HarmonyMethod(typeof(IdTrace), nameof(Postfix)));
        }
        catch (Exception e)
        {
            Health.Failed("patch GetNewUniqueId", e);
        }
    }

    public static IEnumerator Step(string arg, Runner runner)
    {
        if (arg == "on")
        {
            Ids.Clear();
            _on = true;
            yield break;
        }
        if (arg != null && arg.StartsWith("write:", StringComparison.Ordinal))
        {
            _on = false;
            var label = arg.Substring(6);
            using (var j = new Json(Path.Combine(runner.Plan.Out, "ids-" + label + ".json"), indent: true))
            {
                j.Obj();
                j.Prop("schema", "wrath-oracle-ids-v1");
                j.Arr("ids");
                foreach (var kv in Ids)
                {
                    j.Obj();
                    j.Prop("id", kv.Key);
                    j.Arr("stack");
                    foreach (var f in kv.Value) j.W.WriteValue(f);
                    j.EndArr();
                    j.End();
                }
                j.EndArr();
                j.End();
            }
            Ids.Clear();
            yield break;
        }
        throw new ArgumentException("idtrace:on or idtrace:write:LABEL");
    }
}
