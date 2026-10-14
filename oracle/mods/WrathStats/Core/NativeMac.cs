using System;
using System.Runtime.InteropServices;
using System.Text;

namespace WrathStats.Core;

// macOS system queries through libSystem. Every call is guarded: on any other platform,
// or if a symbol is missing, the result is "unknown" rather than an exception.
internal static class NativeMac
{
    private const string LibSystem = "libSystem.dylib";
    private static readonly IntPtr RtldDefault = new IntPtr(-2);
    private const int MachTaskBasicInfo = 20; // MACH_TASK_BASIC_INFO

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    private struct MachTaskBasicInfoData
    {
        public ulong VirtualSize;
        public ulong ResidentSize;
        public ulong ResidentSizeMax;
        public int UserSeconds;
        public int UserMicroseconds;
        public int SystemSeconds;
        public int SystemMicroseconds;
        public int Policy;
        public int SuspendCount;
    }

    [DllImport(LibSystem)]
    private static extern int sysctlbyname(string name, out int value, ref IntPtr size, IntPtr newValue, IntPtr newSize);

    [DllImport(LibSystem, EntryPoint = "sysctlbyname")]
    private static extern int sysctlbyname_bytes(string name, byte[] value, ref IntPtr size, IntPtr newValue, IntPtr newSize);

    [DllImport(LibSystem)]
    private static extern IntPtr dlsym(IntPtr handle, string symbol);

    [DllImport(LibSystem)]
    private static extern int task_info(uint task, int flavor, out MachTaskBasicInfoData info, ref uint count);

    private static readonly uint TaskInfoCount = (uint)(Marshal.SizeOf(typeof(MachTaskBasicInfoData)) / sizeof(int));
    private static bool _taskResolved;
    private static uint _task;

    // 1 under Rosetta, 0 native, null if the sysctl doesn't exist (Intel Macs: native).
    public static int? ProcTranslated()
    {
        try
        {
            var size = (IntPtr)sizeof(int);
            return sysctlbyname("sysctl.proc_translated", out var v, ref size, IntPtr.Zero, IntPtr.Zero) == 0 ? v : (int?)null;
        }
        catch (Exception)
        {
            return null;
        }
    }

    public static string SysctlString(string name)
    {
        try
        {
            var size = IntPtr.Zero;
            if (sysctlbyname_bytes(name, null, ref size, IntPtr.Zero, IntPtr.Zero) != 0 || size == IntPtr.Zero) return null;
            var buf = new byte[(int)size];
            if (sysctlbyname_bytes(name, buf, ref size, IntPtr.Zero, IntPtr.Zero) != 0) return null;
            return Encoding.UTF8.GetString(buf, 0, Math.Max(0, (int)size - 1)).TrimEnd('\0');
        }
        catch (Exception)
        {
            return null;
        }
    }

    public static int? SysctlInt(string name)
    {
        try
        {
            var size = (IntPtr)sizeof(int);
            return sysctlbyname(name, out var v, ref size, IntPtr.Zero, IntPtr.Zero) == 0 ? v : (int?)null;
        }
        catch (Exception)
        {
            return null;
        }
    }

    // Resident set size in bytes, or -1. Called once a second; doesn't allocate.
    public static long ResidentBytes()
    {
        try
        {
            if (!_taskResolved)
            {
                _taskResolved = true;
                // mach_task_self() is a macro over this global; reading it takes no new
                // port reference (unlike task_self_trap).
                var p = dlsym(RtldDefault, "mach_task_self_");
                _task = p == IntPtr.Zero ? 0 : (uint)Marshal.ReadInt32(p);
            }
            if (_task == 0) return -1;
            var count = TaskInfoCount;
            return task_info(_task, MachTaskBasicInfo, out var info, ref count) == 0 ? (long)info.ResidentSize : -1;
        }
        catch (Exception)
        {
            _task = 0;
            return -1;
        }
    }
}
