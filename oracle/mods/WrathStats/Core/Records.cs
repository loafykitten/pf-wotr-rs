using System.Threading;

namespace WrathStats.Core;

// Single-producer (Unity main thread), single-consumer (writer thread) queue over a
// preallocated array. Pushing never blocks or allocates; when full, the item is dropped
// and counted.
internal sealed class Ring<T> where T : struct
{
    private readonly T[] _items;
    private readonly int _mask;
    private int _head; // next slot the producer writes
    private int _tail; // next slot the consumer reads

    public int Dropped; // producer-side only

    public Ring(int capacityPowerOfTwo)
    {
        _items = new T[capacityPowerOfTwo];
        _mask = capacityPowerOfTwo - 1;
    }

    public bool TryPush(ref T item)
    {
        var head = _head;
        if (head - Volatile.Read(ref _tail) >= _items.Length)
        {
            Dropped++;
            return false;
        }
        _items[head & _mask] = item;
        Volatile.Write(ref _head, head + 1);
        return true;
    }

    public bool TryPop(out T item)
    {
        var tail = _tail;
        if (tail == Volatile.Read(ref _head))
        {
            item = default;
            return false;
        }
        item = _items[tail & _mask];
        _items[tail & _mask] = default; // drop string references
        Volatile.Write(ref _tail, tail + 1);
        return true;
    }
}

// Per-frame flags, also the byte stored per frame in frames.bin.
internal static class FrameFlags
{
    public const byte Game = 1 << 0;      // in-game frame: counted in frame stats
    public const byte Menu = 1 << 1;      // main menu, or before the game exists
    public const byte Loading = 1 << 2;   // a LoadingProcess was running this or last frame
    public const byte Combat = 1 << 3;
    public const byte TurnBased = 1 << 4;
    public const byte Paused = 1 << 5;
    public const byte Gc = 1 << 6;        // GC.CollectionCount(0) went up during the frame
    public const byte External = 1 << 7; // another mod reported work (WrathStatsApi)
}

internal struct FrameRecord
{
    public uint Micros;
    public byte Flags;
    public byte Mode; // GameModeType.Index (see session.json "modes"); 255 = no game
}

internal struct SecondRecord
{
    public double T;  // session seconds at the end of this interval
    public double Dt; // interval length: about 1 s, longer after a stall
    public int Frames;
    public int GameFrames;
    public int MenuFrames;
    public int LoadingFrames;
    public double Mean;
    public double P50;
    public double P95;
    public double P99;
    public double Max;
    public int Over33;
    public int Over50;
    public int GcCount;
    public int GcFrames;
    public int ExternalFrames;
    public long GcTotalMemory;
    public long MonoUsed;
    public long MonoHeap;
    public long UnityAllocated;
    public long UnityReserved;
    public long Resident;
    public string State;
    public string Area;
    public string Mode;
    public bool Combat;
    public bool TurnBased;
    public bool Paused;
    public int AwakeUnits;
    public int Party;
    public float TimeScale;
    public float PlayerTimeScale;
}

internal enum EventKind : byte
{
    Hitch,
    Load,
    Area,
    Marker,
    Note,
}

internal struct EventRecord
{
    public EventKind Kind;
    public double T;      // session seconds (load: when it started)
    public double Ms;     // hitch: frame time; load: duration
    public long Frame;
    // Context (hitch, marker)
    public string State;
    public string Area;
    public string Mode;
    public bool Combat;
    public bool TurnBased;
    public bool Paused;
    public bool Gc;
    public string External;
    public double SinceLoad; // seconds since the last load ended; -1 if none yet
    // Load
    public string Reason;
    public string SaveType;
    public int Tags; // bit per LoadingProcessTag seen during the load
    public string AreaBefore;
    public int Frames;
    public long EndFrame; // load: frame index of its last frame (Frame = its first)
    public double MaxFrameMs;
    public int GcCount;
    // Note
    public string Text;
}
