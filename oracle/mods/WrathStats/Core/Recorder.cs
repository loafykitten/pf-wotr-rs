using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using Kingmaker.Utility;
using UnityEngine;
using UnityEngine.Profiling;
using Debug = UnityEngine.Debug;

namespace WrathStats.Core;

// Measures every frame and writes one session folder per recording:
//   <persistent data>/WrathStats/<UTC yyyyMMddTHHmmssZ>/
//     session.json, seconds.csv, events.jsonl, frames.bin, general_settings.json (copy)
//
// LateUpdate is the hot path: no allocation, a handful of field reads, ring pushes.
// Per-second aggregation also stays allocation-free. Session start/stop and rare events
// (loads, area changes) may allocate.
internal sealed class Recorder : MonoBehaviour
{
    public const string FolderName = "WrathStats";
    private const int MaxGameFramesPerSecond = 4096;

    // Configuration, set by the host before Begin().
    public float HitchMs = 50f;
    public bool MarkerHotkey = true;
    public Action<string> Log = Debug.Log;
    public Action<string> LogError = Debug.LogError;

    // Live numbers for the settings UI (main thread only).
    public string SessionDir { get; private set; }
    public bool Recording => _writer != null;
    public double LastP50, LastP95, LastMax;
    public int LastFps;
    public long Hitches, Loads, Markers;

    private static readonly double TicksToMs = 1000.0 / Stopwatch.Frequency;

    private Writer _writer;
    private GameContext _ctx;
    private long _sessionStartTs, _lastTs, _secondStartTs;
    private long _frame;
    private int _lastGc;

    // Current second
    private readonly double[] _gameMs = new double[MaxGameFramesPerSecond];
    private int _gameCount, _frames, _menuFrames, _loadingFrames, _over33, _over50, _gcCount, _gcFrames, _extFrames;
    private double _sum, _max;

    // Current load span
    private bool _inLoad;
    private long _loadStartTs, _loadStartFrame;
    private string _loadReason, _loadSaveType, _loadAreaBefore;
    private int _loadTags, _loadFrames, _loadGc;
    private double _loadMax;
    private double _lastLoadEndT = -1;

    // Reason reported by a Harmony prefix before its load span begins.
    private string _pendingReason, _pendingSaveType;
    private long _pendingFrame;

    private bool _writerErrorReported;

    // Our own per-frame cost (LateUpdate), reported at session end.
    private long _selfTicks, _selfMaxTicks, _selfFrames;

    public void Begin(string statsVersion, IEnumerable<ModInfo> mods)
    {
        if (_writer != null) return;
        var start = DateTime.UtcNow;
        var root = Path.Combine(ApplicationPaths.persistentDataPath ?? Application.persistentDataPath, FolderName);
        var stamp = start.ToString("yyyyMMdd'T'HHmmss'Z'", CultureInfo.InvariantCulture);
        var dir = Path.Combine(root, stamp);
        for (var n = 2; Directory.Exists(dir); n++) dir = Path.Combine(root, $"{stamp}-{n}");
        Directory.CreateDirectory(dir);

        File.WriteAllText(Path.Combine(dir, "session.json"), SessionInfo.Build(start, statsVersion, mods, HitchMs));
        TryCopy(Path.Combine(ApplicationPaths.persistentDataPath ?? "", "general_settings.json"), Path.Combine(dir, "general_settings.json"));

        _ctx = new GameContext(e => LogError($"game state read failed (logged once): {e}"));
        _writer = new Writer(dir);
        SessionDir = dir;
        LoadReasons.OnReason = OnReason;
        QuitHook.OnQuit = End;

        _sessionStartTs = _lastTs = _secondStartTs = Stopwatch.GetTimestamp();
        _frame = 0;
        _lastGc = GC.CollectionCount(0);
        ResetSecond();
        _inLoad = false;
        _lastLoadEndT = -1;
        _pendingReason = null;
        Hitches = Loads = Markers = 0;
        _selfTicks = _selfMaxTicks = _selfFrames = 0;
        _lastLoadReason = null;
        _writerErrorReported = false;
        Note("session start");
        Log($"recording to {dir}");
    }

    public void End()
    {
        if (_writer == null) return;
        LoadReasons.OnReason = null;
        QuitHook.OnQuit = null;
        var now = Stopwatch.GetTimestamp();
        if (_inLoad) FinishLoad(now, interrupted: true);
        if (_frames > 0) FinishSecond(now);
        var selfMeanUs = _selfFrames > 0 ? _selfTicks * TicksToMs * 1000.0 / _selfFrames : 0;
        Note($"self cost per frame: mean {selfMeanUs:0.0} us, max {_selfMaxTicks * TicksToMs * 1000.0:0} us over {_selfFrames} frames");
        Note($"session end; dropped frames={_writer.Frames.Dropped} seconds={_writer.Seconds.Dropped} events={_writer.Events.Dropped}");
        _writer.Dispose();
        ReportWriterError();
        _writer = null;
        Log($"stopped recording ({SessionDir})");
    }

    public void AddMarker()
    {
        if (_writer == null) return;
        var e = ContextEvent(EventKind.Marker, Stopwatch.GetTimestamp(), 0, gc: false, external: null);
        _writer.Events.TryPush(ref e);
        Markers++;
    }

    private void OnApplicationQuit() => End();

    private void OnDestroy() => End();

    private void LateUpdate()
    {
        if (_writer == null) return;
        var now = Stopwatch.GetTimestamp();
        var ms = (now - _lastTs) * TicksToMs;
        _lastTs = now;
        _frame++;

        var gc = GC.CollectionCount(0);
        var gcHit = gc != _lastGc;
        if (gcHit)
        {
            _gcCount += gc - _lastGc;
            _gcFrames++;
        }
        _lastGc = gc;

        _ctx.UpdateFrame();
        var external = WrathStatsApi.ConsumeFrame(out var externalSource);

        byte flags = 0;
        if (_ctx.Loading) flags |= FrameFlags.Loading;
        else if (_ctx.Menu) flags |= FrameFlags.Menu;
        else flags |= FrameFlags.Game;
        if (_ctx.Combat) flags |= FrameFlags.Combat;
        if (_ctx.TurnBased) flags |= FrameFlags.TurnBased;
        if (_ctx.Paused) flags |= FrameFlags.Paused;
        if (gcHit) flags |= FrameFlags.Gc;
        if (external) flags |= FrameFlags.External;

        var frame = new FrameRecord
        {
            Micros = (uint)Math.Min(Math.Round(ms * 1000.0), uint.MaxValue),
            Flags = flags,
            Mode = (byte)(_ctx.ModeIndex < 0 || _ctx.ModeIndex > 255 ? 255 : _ctx.ModeIndex),
        };
        _writer.Frames.TryPush(ref frame);

        _frames++;
        if (external) _extFrames++;
        if ((flags & FrameFlags.Game) != 0)
        {
            if (_gameCount < _gameMs.Length) _gameMs[_gameCount++] = ms;
            _sum += ms;
            if (ms > _max) _max = ms;
            if (ms > 33.3) _over33++;
            if (ms > 50.0) _over50++;
            if (ms >= HitchMs)
            {
                var e = ContextEvent(EventKind.Hitch, now, ms, gcHit, externalSource);
                _writer.Events.TryPush(ref e);
                Hitches++;
            }
        }
        else if ((flags & FrameFlags.Loading) != 0)
        {
            _loadingFrames++;
        }
        else
        {
            _menuFrames++;
        }

        TrackLoad(now, ms, gcHit);

        if (_ctx.AreaChanged)
        {
            var e = new EventRecord
            {
                Kind = EventKind.Area,
                T = Seconds(now),
                AreaBefore = _ctx.PreviousAreaName,
                Area = _ctx.AreaName,
            };
            _writer.Events.TryPush(ref e);
        }

        if (MarkerHotkey) CheckMarkerHotkey();
        while (WrathStatsApi.TryTakeNote(out var noteText)) Note(noteText);

        if (now - _secondStartTs >= Stopwatch.Frequency) FinishSecond(now);

        var self = Stopwatch.GetTimestamp() - now;
        _selfTicks += self;
        _selfFrames++;
        if (self > _selfMaxTicks) _selfMaxTicks = self;
    }

    private void CheckMarkerHotkey()
    {
        try
        {
            if (Input.GetKeyDown(KeyCode.M)
                && (Input.GetKey(KeyCode.LeftControl) || Input.GetKey(KeyCode.RightControl))
                && (Input.GetKey(KeyCode.LeftShift) || Input.GetKey(KeyCode.RightShift)))
            {
                AddMarker();
            }
        }
        catch (Exception e)
        {
            // e.g. legacy Input disabled: don't throw every frame.
            MarkerHotkey = false;
            LogError($"marker hotkey disabled: {e.Message}");
        }
    }

    private void OnReason(string reason, string saveType)
    {
        if (_inLoad)
        {
            // e.g. an autosave queued inside an area transition: keep the first reason.
            if (_loadReason == null) _loadReason = reason;
            if (saveType != null) _loadSaveType = saveType;
            return;
        }
        // Prefixes run during Update; the span is noticed in this frame's LateUpdate. Within
        // one frame the higher-ranked reason wins (e.g. new_game over the area_transition it
        // triggers, or over a save queued alongside); a save type is always kept.
        if (_pendingReason != null && _pendingFrame == _frame && LoadReasons.Rank(reason) < LoadReasons.Rank(_pendingReason))
        {
            if (saveType != null) _pendingSaveType = saveType;
            return;
        }
        _pendingReason = reason;
        _pendingSaveType = saveType;
        _pendingFrame = _frame;
    }

    private void TrackLoad(long now, double ms, bool gcHit)
    {
        // Every patched entry point starts its LoadingProcess synchronously, so a reason
        // whose load didn't start by the next frame was refused (e.g. saving in combat).
        if (_pendingReason != null && !_inLoad && _frame - _pendingFrame > 1) _pendingReason = _pendingSaveType = null;

        if (_ctx.LoadingNow && !_inLoad)
        {
            _inLoad = true;
            _loadStartTs = now;
            _loadStartFrame = _frame;
            _loadReason = _pendingReason;
            _loadSaveType = _pendingSaveType;
            _pendingReason = _pendingSaveType = null;
            _loadAreaBefore = _ctx.AreaName;
            _loadTags = 0;
            _loadFrames = 0;
            _loadGc = 0;
            _loadMax = 0;
        }
        if (!_inLoad) return;

        if (_ctx.LoadingNow)
        {
            if (_ctx.LoadingTag >= 0 && _ctx.LoadingTag < 31) _loadTags |= 1 << _ctx.LoadingTag;
            _loadFrames++;
            if (gcHit) _loadGc++;
            if (ms > _loadMax) _loadMax = ms;
        }
        else
        {
            // Loading finished during this frame's Update: its cost belongs to the load.
            _loadFrames++;
            if (gcHit) _loadGc++;
            if (ms > _loadMax) _loadMax = ms;
            FinishLoad(now, interrupted: false);
        }
    }

    private void FinishLoad(long now, bool interrupted)
    {
        _inLoad = false;
        var e = new EventRecord
        {
            Kind = EventKind.Load,
            T = Seconds(_loadStartTs),
            Ms = (now - _loadStartTs) * TicksToMs,
            Reason = interrupted ? "interrupted" : _loadReason ?? "unknown",
            SaveType = _loadSaveType,
            Tags = _loadTags,
            AreaBefore = _loadAreaBefore,
            Area = _ctx.AreaName,
            Frames = _loadFrames,
            Frame = _loadStartFrame,
            EndFrame = _frame,
            MaxFrameMs = _loadMax,
            GcCount = _loadGc,
        };
        _writer.Events.TryPush(ref e);
        _lastLoadEndT = Seconds(now);
        Loads++;
        _lastLoadMs = e.Ms;
        _lastLoadReason = e.Reason;
    }

    private double _lastLoadMs;
    private string _lastLoadReason;

    public string LastLoadText() => _lastLoadReason == null ? "none yet" : $"{_lastLoadReason}: {_lastLoadMs / 1000.0:0.00} s";

    private void FinishSecond(long now)
    {
        var r = new SecondRecord
        {
            T = Seconds(now),
            Dt = (now - _secondStartTs) / (double)Stopwatch.Frequency,
            Frames = _frames,
            GameFrames = _gameCount,
            MenuFrames = _menuFrames,
            LoadingFrames = _loadingFrames,
            Over33 = _over33,
            Over50 = _over50,
            GcCount = _gcCount,
            GcFrames = _gcFrames,
            ExternalFrames = _extFrames,
            State = _loadingFrames > 0 ? "loading" : _menuFrames > _gameCount ? "menu" : "game",
        };
        if (_gameCount > 0)
        {
            HeapSort(_gameMs, _gameCount);
            r.Mean = _sum / _gameCount;
            r.P50 = Percentile(0.50);
            r.P95 = Percentile(0.95);
            r.P99 = Percentile(0.99);
            r.Max = _max;
        }
        r.GcTotalMemory = GC.GetTotalMemory(false);
        r.MonoUsed = Profiler.GetMonoUsedSizeLong();
        r.MonoHeap = Profiler.GetMonoHeapSizeLong();
        r.UnityAllocated = Profiler.GetTotalAllocatedMemoryLong();
        r.UnityReserved = Profiler.GetTotalReservedMemoryLong();
        r.Resident = NativeMac.ResidentBytes();
        _ctx.ReadSecond(ref r);
        _writer.Seconds.TryPush(ref r);
        _writer.Wake();

        LastFps = _frames;
        LastP50 = r.P50;
        LastP95 = r.P95;
        LastMax = r.Max;
        ResetSecond();
        // After a long stall, start a fresh second instead of emitting empty ones.
        _secondStartTs = now;
        ReportWriterError();
    }

    // In-place, allocation-free. Mono's Array.Sort<T> allocates a Comparison delegate per call.
    private static void HeapSort(double[] a, int n)
    {
        for (var i = n / 2 - 1; i >= 0; i--) SiftDown(a, i, n);
        for (var end = n - 1; end > 0; end--)
        {
            var t = a[0];
            a[0] = a[end];
            a[end] = t;
            SiftDown(a, 0, end);
        }
    }

    private static void SiftDown(double[] a, int root, int n)
    {
        var v = a[root];
        while (true)
        {
            var child = 2 * root + 1;
            if (child >= n) break;
            if (child + 1 < n && a[child + 1] > a[child]) child++;
            if (a[child] <= v) break;
            a[root] = a[child];
            root = child;
        }
        a[root] = v;
    }

    private double Percentile(double p)
    {
        var idx = (int)Math.Ceiling(p * _gameCount) - 1;
        return _gameMs[Math.Max(0, Math.Min(_gameCount - 1, idx))];
    }

    private void ResetSecond()
    {
        _gameCount = _frames = _menuFrames = _loadingFrames = _over33 = _over50 = _gcCount = _gcFrames = _extFrames = 0;
        _sum = _max = 0;
    }

    private EventRecord ContextEvent(EventKind kind, long now, double ms, bool gc, string external) => new EventRecord
    {
        Kind = kind,
        T = Seconds(now),
        Ms = ms,
        Frame = _frame,
        State = _ctx.Loading ? "loading" : _ctx.Menu ? "menu" : "game",
        Area = _ctx.AreaName,
        Mode = _ctx.ModeName(),
        Combat = _ctx.Combat,
        TurnBased = _ctx.TurnBased,
        Paused = _ctx.Paused,
        Gc = gc,
        External = external,
        SinceLoad = _lastLoadEndT < 0 ? -1 : Seconds(now) - _lastLoadEndT,
    };

    private void Note(string text)
    {
        var e = new EventRecord { Kind = EventKind.Note, T = Seconds(Stopwatch.GetTimestamp()), Text = text };
        _writer.Events.TryPush(ref e);
    }

    private double Seconds(long ts) => (ts - _sessionStartTs) / (double)Stopwatch.Frequency;

    private void ReportWriterError()
    {
        if (_writerErrorReported || _writer?.Error == null) return;
        _writerErrorReported = true;
        LogError($"writer error: {_writer.Error}");
    }

    private static void TryCopy(string from, string to)
    {
        try
        {
            if (File.Exists(from)) File.Copy(from, to, true);
        }
        catch (Exception)
        {
            // Optional extra; ignore.
        }
    }
}
