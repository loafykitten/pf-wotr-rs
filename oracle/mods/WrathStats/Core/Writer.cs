using System;
using System.IO;
using System.Text;
using System.Threading;

namespace WrathStats.Core;

// Owns a session's output files. The main thread only pushes into the rings and calls
// Wake() once a second; this thread does all formatting and I/O. Formatting goes through
// LineBuilder, so steady-state writing doesn't allocate either.
internal sealed class Writer : IDisposable
{
    public const string SecondsHeader =
        "t_s,dt_s,state,frames,game_frames,menu_frames,loading_frames,mean_ms,p50_ms,p95_ms,p99_ms,max_ms," +
        "over33,over50,gc_count,gc_frames,external_frames,gc_total_bytes,mono_used_bytes,mono_heap_bytes," +
        "unity_allocated_bytes,unity_reserved_bytes,resident_bytes,area,mode,combat,turn_based,paused," +
        "awake_units,party,time_scale,player_time_scale";

    // frames.bin: this 8-byte magic, then 6 bytes per frame: uint32 LE microseconds,
    // flags byte (FrameFlags), game mode byte (GameModeType.Index; 255 = no game).
    // WSFRAME1 files (WrathStats 0.1.x) have no mode byte.
    public static readonly byte[] FramesMagic = Encoding.ASCII.GetBytes("WSFRAME2");
    private const int FrameSize = 6;

    // LoadingProcessTag names, by value.
    private static readonly string[] TagNames =
        { "None", "Save", "ReloadMechanics", "ReloadLight", "Camping", "TeleportParty", "ShowCityBuilder", "Load" };

    public readonly Ring<FrameRecord> Frames = new Ring<FrameRecord>(1 << 14);
    public readonly Ring<SecondRecord> Seconds = new Ring<SecondRecord>(1 << 8);
    public readonly Ring<EventRecord> Events = new Ring<EventRecord>(1 << 10);

    private readonly Thread _thread;
    private readonly AutoResetEvent _wake = new AutoResetEvent(false);
    private readonly StreamWriter _seconds;
    private readonly StreamWriter _events;
    private readonly FileStream _frames;
    private readonly byte[] _frameBuf = new byte[FrameSize * 4096];
    private readonly LineBuilder _line = new LineBuilder();
    private volatile bool _stop;
    private bool _disposed;

    public volatile Exception Error; // first I/O error; the recorder reports it on the main thread

    public Writer(string dir)
    {
        var utf8 = new UTF8Encoding(false);
        _seconds = new StreamWriter(Path.Combine(dir, "seconds.csv"), false, utf8);
        _seconds.WriteLine(SecondsHeader);
        _events = new StreamWriter(Path.Combine(dir, "events.jsonl"), false, utf8);
        _frames = new FileStream(Path.Combine(dir, "frames.bin"), FileMode.Create, FileAccess.Write, FileShare.Read);
        _frames.Write(FramesMagic, 0, FramesMagic.Length);

        _thread = new Thread(Run) { Name = "WrathStats writer", IsBackground = true, Priority = ThreadPriority.BelowNormal };
        _thread.Start();
    }

    public void Wake() => _wake.Set();

    // Main thread: stop and flush everything that was queued. Blocks briefly.
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _stop = true;
        _wake.Set();
        if (!_thread.Join(3000))
            Error = Error ?? new TimeoutException("writer thread did not stop within 3 s");
    }

    private void Run()
    {
        while (!_stop)
        {
            _wake.WaitOne(1000);
            Drain();
        }
        Drain();
        try
        {
            _seconds.Dispose();
            _events.Dispose();
            _frames.Dispose();
        }
        catch (Exception e)
        {
            Error = Error ?? e;
        }
    }

    private void Drain()
    {
        try
        {
            DrainFrames();
            while (Seconds.TryPop(out var s)) WriteSecond(ref s);
            while (Events.TryPop(out var e)) WriteEvent(ref e);
            _seconds.Flush();
            _events.Flush();
            _frames.Flush();
        }
        catch (Exception e)
        {
            Error = Error ?? e;
        }
    }

    private void DrainFrames()
    {
        var n = 0;
        while (Frames.TryPop(out var f))
        {
            _frameBuf[n++] = (byte)f.Micros;
            _frameBuf[n++] = (byte)(f.Micros >> 8);
            _frameBuf[n++] = (byte)(f.Micros >> 16);
            _frameBuf[n++] = (byte)(f.Micros >> 24);
            _frameBuf[n++] = f.Flags;
            _frameBuf[n++] = f.Mode;
            if (n == _frameBuf.Length)
            {
                _frames.Write(_frameBuf, 0, n);
                n = 0;
            }
        }
        if (n > 0) _frames.Write(_frameBuf, 0, n);
    }

    private void WriteSecond(ref SecondRecord s)
    {
        var l = _line;
        l.Clear();
        l.Append(s.T, 3).Append(',').Append(s.Dt, 3).Append(',').Append(s.State).Append(',')
            .Append(s.Frames).Append(',').Append(s.GameFrames).Append(',')
            .Append(s.MenuFrames).Append(',').Append(s.LoadingFrames).Append(',');
        if (s.GameFrames > 0)
        {
            l.Append(s.Mean, 3).Append(',').Append(s.P50, 3).Append(',').Append(s.P95, 3).Append(',')
                .Append(s.P99, 3).Append(',').Append(s.Max, 3).Append(',');
        }
        else
        {
            l.Append(",,,,,");
        }
        l.Append(s.Over33).Append(',').Append(s.Over50).Append(',')
            .Append(s.GcCount).Append(',').Append(s.GcFrames).Append(',').Append(s.ExternalFrames).Append(',')
            .Append(s.GcTotalMemory).Append(',').Append(s.MonoUsed).Append(',').Append(s.MonoHeap).Append(',')
            .Append(s.UnityAllocated).Append(',').Append(s.UnityReserved).Append(',').Append(s.Resident).Append(',')
            .AppendCsv(s.Area).Append(',').AppendCsv(s.Mode).Append(',')
            .Append(s.Combat).Append(',').Append(s.TurnBased).Append(',').Append(s.Paused).Append(',')
            .Append(s.AwakeUnits).Append(',').Append(s.Party).Append(',')
            .Append(s.TimeScale, 3).Append(',').Append(s.PlayerTimeScale, 3).Append('\n');
        l.WriteTo(_seconds);
    }

    private void WriteEvent(ref EventRecord e)
    {
        var l = _line;
        l.Clear();
        l.Append("{\"t\":").Append(e.T, 3);
        switch (e.Kind)
        {
            case EventKind.Hitch:
                l.Append(",\"kind\":\"hitch\",\"ms\":").Append(e.Ms, 2);
                AppendContext(ref e);
                break;
            case EventKind.Marker:
                l.Append(",\"kind\":\"marker\"");
                AppendContext(ref e);
                break;
            case EventKind.Load:
                l.Append(",\"kind\":\"load\",\"ms\":").Append(e.Ms, 1)
                    .Append(",\"reason\":").AppendJson(e.Reason)
                    .Append(",\"save_type\":").AppendJson(e.SaveType)
                    .Append(",\"tags\":[");
                var first = true;
                for (var i = 0; i < TagNames.Length; i++)
                {
                    if ((e.Tags & (1 << i)) == 0) continue;
                    if (!first) l.Append(',');
                    l.AppendJson(TagNames[i]);
                    first = false;
                }
                l.Append("],\"area_before\":").AppendJson(e.AreaBefore)
                    .Append(",\"area\":").AppendJson(e.Area)
                    .Append(",\"frames\":").Append(e.Frames)
                    .Append(",\"frame\":").Append(e.Frame)
                    .Append(",\"end_frame\":").Append(e.EndFrame)
                    .Append(",\"max_frame_ms\":").Append(e.MaxFrameMs, 2)
                    .Append(",\"gc\":").Append((long)e.GcCount);
                break;
            case EventKind.Area:
                l.Append(",\"kind\":\"area\",\"from\":").AppendJson(e.AreaBefore)
                    .Append(",\"to\":").AppendJson(e.Area);
                break;
            case EventKind.Note:
                l.Append(",\"kind\":\"note\",\"text\":").AppendJson(e.Text);
                break;
        }
        l.Append("}\n");
        l.WriteTo(_events);
    }

    private void AppendContext(ref EventRecord e)
    {
        _line.Append(",\"frame\":").Append(e.Frame)
            .Append(",\"state\":").AppendJson(e.State)
            .Append(",\"area\":").AppendJson(e.Area)
            .Append(",\"mode\":").AppendJson(e.Mode)
            .Append(",\"combat\":").Append(e.Combat ? "true" : "false")
            .Append(",\"turn_based\":").Append(e.TurnBased ? "true" : "false")
            .Append(",\"paused\":").Append(e.Paused ? "true" : "false")
            .Append(",\"gc\":").Append(e.Gc ? "true" : "false")
            .Append(",\"external\":").AppendJson(e.External)
            .Append(",\"since_load_s\":");
        if (e.SinceLoad < 0) _line.Append("null");
        else _line.Append(e.SinceLoad, 1);
    }
}
