using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Text;
using System.Threading;
using UnityEngine;

namespace WrathOracle;

// The fixture trace (WrathOracle 0.2): one ordered stream of JSON-lines records for a
// fixture window: ticks and time, rules (nested, with handlers and mutations), every hooked
// random draw, dialogue transitions, inputs. Every record has
//   "seq": global sequence number (no gaps; a record that failed to build is replaced by a
//          {"k":"failed"} record with the same seq), "k": kind, "t": tick (game-mode ticks
//          since the window began, -1 before the first)
// Records are built in memory on the calling thread (main thread except draws during
// loading) and written by a background thread through gzip. Nothing here may trigger a
// rule, an action or a random draw.
internal static class Trace
{
    public static volatile bool Active;
    public static long Seq;
    public static int Tick = -1;
    public static string Name;
    public static string Mode = "record"; // record | replay
    public static readonly int MainThread = Thread.CurrentThread.ManagedThreadId;

    // Health of the current window (also folded into the global Health counters).
    public static long Written, Failed, Dropped, StructuralErrors;
    private static readonly List<string> Errors = new List<string>();

    private static Writer _writer;
    private static readonly object EmitLock = new object();

    public static bool OnMainThread => Thread.CurrentThread.ManagedThreadId == MainThread;

    public static void Begin(string path, string name, string mode, Action<Json> header)
    {
        End(null);
        Seq = 0;
        Tick = -1;
        Written = Failed = Dropped = StructuralErrors = HookFailures = 0;
        Snap.Truncated = Snap.Cycles = Snap.ValueFailures = 0;
        lock (Errors) Errors.Clear();
        Name = name;
        Mode = mode;
        _writer = new Writer(path);
        Rules.Reset();
        Active = true;
        Emit("begin", j =>
        {
            j.Prop("schema", "wrath-oracle-trace-v1");
            j.Prop("name", name);
            j.Prop("mode", mode);
            j.Prop("frame", Time.frameCount);
            j.Prop("random_state", RandomState.Text());
            header?.Invoke(j);
        });
    }

    public static void End(Action<Json> footer)
    {
        if (_writer == null) return;
        Rules.CheckStructureAtEnd();
        Emit("end", j =>
        {
            j.Prop("frame", Time.frameCount);
            j.Prop("random_state", RandomState.Text());
            footer?.Invoke(j);
            WriteHealth(j);
        });
        Active = false;
        _writer.Close();
        _writer = null;
    }

    public static void WriteHealth(Json j)
    {
        j.Obj("health");
        j.Prop("records_written", Interlocked.Read(ref Written));
        j.Prop("records_failed", Interlocked.Read(ref Failed));
        j.Prop("records_dropped", Interlocked.Read(ref Dropped));
        j.Prop("structural_errors", Interlocked.Read(ref StructuralErrors));
        j.Prop("hook_exceptions_caught", Interlocked.Read(ref HookFailures));
        j.Prop("rng_unhooked_steps", Rng.UnhookedSteps);
        j.Prop("rng_unhooked_unknown", Rng.UnhookedUnknown);
        j.Prop("rng_global_hook_draws", Rng.GlobalHookDraws);
        j.Prop("rng_offthread_draws", Rng.OffThreadDraws);
        j.Prop("replay_mismatches", Rng.ReplayMismatches);
        j.Prop("replay_tape_exhausted", Rng.ReplayExhausted);
        j.Prop("snapshot_truncated", Snap.Truncated);
        j.Prop("snapshot_value_failures", Snap.ValueFailures);
        j.Arr("errors");
        lock (Errors) foreach (var e in Errors) j.W.WriteValue(e);
        j.EndArr();
        j.End();
    }

    public static long HookFailures;

    // A hook body threw: count it ourselves (the game's RulebookEventHooksManager, subscriber
    // lists and controllers swallow exceptions, so nobody else would notice).
    public static void HookFailed(string where, Exception e)
    {
        Interlocked.Increment(ref HookFailures);
        Interlocked.Increment(ref Failed);
        Health.Failed("hook " + where, e);
        AddError($"hook {where}: {e.GetType().Name}: {e.Message}");
    }

    public static void Structural(string message)
    {
        Interlocked.Increment(ref StructuralErrors);
        AddError("structure: " + message);
    }

    private static void AddError(string e)
    {
        lock (Errors)
        {
            if (Errors.Count < 200) Errors.Add(e);
        }
    }

    // Builds and queues one record. The body writes the record's own properties.
    public static void Emit(string kind, Action<Json> body)
    {
        if (_writer == null) return;
        string text;
        long seq;
        lock (EmitLock)
        {
            seq = Seq++;
            try
            {
                var sw = new StringWriter(CultureInfo.InvariantCulture);
                using (var j = new Json(sw))
                {
                    j.Obj();
                    j.Prop("seq", seq);
                    j.Prop("k", kind);
                    j.Prop("t", Tick);
                    if (!OnMainThread) j.Prop("thread", Thread.CurrentThread.ManagedThreadId);
                    body?.Invoke(j);
                    j.End();
                }
                text = sw.ToString();
            }
            catch (Exception e)
            {
                Interlocked.Increment(ref Failed);
                Health.Failed("trace " + kind, e);
                AddError($"record {seq} ({kind}): {e.GetType().Name}: {e.Message}");
                text = "{\"seq\":" + seq.ToString(CultureInfo.InvariantCulture) + ",\"k\":\"failed\",\"t\":" + Tick.ToString(CultureInfo.InvariantCulture) +
                       ",\"what\":" + Newtonsoft.Json.JsonConvert.ToString(kind + ": " + e.GetType().Name + ": " + e.Message) + "}";
            }
            if (_writer.Enqueue(text))
            {
                Interlocked.Increment(ref Written);
                Health.Written();
            }
            else
            {
                Interlocked.Increment(ref Dropped);
                Health.Dropped();
            }
        }
    }

    // Background writer: gzip if the platform's zlib is available, plain text otherwise.
    private sealed class Writer
    {
        private const int Capacity = 4_000_000; // records queued but not yet written
        private readonly Queue<string> _queue = new Queue<string>();
        private readonly Thread _thread;
        private readonly Stream _file;
        private readonly Stream _gz;
        private readonly StreamWriter _text;
        private bool _closing;

        public Writer(string path)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            _file = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read, 1 << 20);
            try
            {
                _gz = new GZipStream(_file, CompressionMode.Compress, leaveOpen: true);
            }
            catch (Exception e)
            {
                Health.Failed("gzip unavailable; writing plain text", e);
                _gz = null;
            }
            _text = new StreamWriter(_gz ?? _file, new UTF8Encoding(false), 1 << 20);
            _thread = new Thread(Run) { IsBackground = true, Name = "WrathOracle trace writer" };
            _thread.Start();
        }

        public bool Enqueue(string line)
        {
            lock (_queue)
            {
                if (_queue.Count >= Capacity) return false;
                _queue.Enqueue(line);
                if (_queue.Count == 1) Monitor.Pulse(_queue);
                return true;
            }
        }

        private void Run()
        {
            var batch = new List<string>(4096);
            while (true)
            {
                lock (_queue)
                {
                    while (_queue.Count == 0 && !_closing) Monitor.Wait(_queue);
                    if (_queue.Count == 0 && _closing) break;
                    while (_queue.Count > 0 && batch.Count < 4096) batch.Add(_queue.Dequeue());
                }
                try
                {
                    foreach (var line in batch)
                    {
                        _text.Write(line);
                        _text.Write('\n');
                    }
                }
                catch (Exception e)
                {
                    Interlocked.Add(ref Dropped, batch.Count);
                    Health.Failed("trace write", e);
                }
                batch.Clear();
            }
        }

        public void Close()
        {
            lock (_queue)
            {
                _closing = true;
                Monitor.PulseAll(_queue);
            }
            _thread.Join();
            _text.Flush();
            _text.Dispose(); // closes the gzip stream (writes its trailer) and the file
            if (_gz != null) _file.Dispose();
        }
    }
}
