using System;
using System.Runtime.InteropServices;
using UnityEngine.Profiling;

namespace WrathOracle;

// Memory at fixed points of a plan (step "mem:LABEL"). Resident size from
// mach_task_basic_info (as WrathStats reads it), plus Mono and Unity profiler counters.
internal static class Memory
{
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    private struct MachTaskBasicInfoData
    {
        public ulong VirtualSize;
        public ulong ResidentSize;
        public ulong ResidentSizeMax;
        public int UserSeconds, UserMicroseconds, SystemSeconds, SystemMicroseconds, Policy, SuspendCount;
    }

    [DllImport("libSystem.dylib")]
    private static extern IntPtr dlsym(IntPtr handle, string symbol);

    [DllImport("libSystem.dylib")]
    private static extern int task_info(uint task, int flavor, out MachTaskBasicInfoData info, ref uint count);

    private static (long resident, long residentMax) Resident()
    {
        try
        {
            var p = dlsym(new IntPtr(-2), "mach_task_self_");
            if (p == IntPtr.Zero) return (-1, -1);
            var task = (uint)Marshal.ReadInt32(p);
            var count = (uint)(Marshal.SizeOf(typeof(MachTaskBasicInfoData)) / sizeof(int));
            return task_info(task, 20, out var info, ref count) == 0 ? ((long)info.ResidentSize, (long)info.ResidentSizeMax) : (-1, -1);
        }
        catch (Exception)
        {
            return (-1, -1);
        }
    }

    public static void Write(Json j, string label)
    {
        var (res, resMax) = Resident();
        j.Obj();
        j.Prop("label", label);
        j.Prop("utc", DateTime.UtcNow.ToString("o"));
        j.Prop("realtime_s", UnityEngine.Time.realtimeSinceStartup);
        j.Prop("resident_bytes", res);
        j.Prop("resident_max_bytes", resMax);
        j.Prop("mono_used_bytes", Profiler.GetMonoUsedSizeLong());
        j.Prop("mono_heap_bytes", Profiler.GetMonoHeapSizeLong());
        j.Prop("gc_total_memory_bytes", GC.GetTotalMemory(false));
        j.Prop("unity_allocated_bytes", Profiler.GetTotalAllocatedMemoryLong());
        j.Prop("unity_reserved_bytes", Profiler.GetTotalReservedMemoryLong());
        j.Prop("gfx_driver_allocated_bytes", Profiler.GetAllocatedMemoryForGraphicsDriver());
        j.Prop("gc_collections_gen0", GC.CollectionCount(0));
        j.End();
    }
}
