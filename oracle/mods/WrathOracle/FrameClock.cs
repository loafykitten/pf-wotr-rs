using System;
using System.Diagnostics;
using UnityEngine;
using UnityEngine.Rendering;

namespace WrathOracle;

// Frame intervals while armed: Stopwatch timestamps taken in LateUpdate (the same measure
// as WrathStats: the interval between consecutive LateUpdates), preallocated, no
// allocation per frame. Frames past the buffer are counted as dropped.
internal sealed class FrameClock : MonoBehaviour
{
    public static FrameClock Instance { get; private set; }
    private const int Capacity = 1 << 18; // 262,144 frames: 18 min at 240 fps

    private readonly long[] _ts = new long[Capacity];
    private readonly byte[] _gc = new byte[Capacity];
    private int _count;
    private bool _armed;
    private int _lastGc;
    public long Dropped;

    private void Awake() => Instance = this;

    public void Arm()
    {
        _count = 0;
        Dropped = 0;
        _lastGc = GC.CollectionCount(0);
        _armed = true;
    }

    public void Disarm() => _armed = false;

    public int Count => _count;

    // Interval i (between timestamps i-1 and i) in milliseconds, i >= 1.
    public double IntervalMs(int i) => (_ts[i] - _ts[i - 1]) * 1000.0 / Stopwatch.Frequency;
    public long Timestamp(int i) => _ts[i];
    public bool GcAt(int i) => _gc[i] != 0;

    private void LateUpdate()
    {
        if (!_armed) return;
        if (_count >= Capacity)
        {
            Dropped++;
            return;
        }
        var gc = GC.CollectionCount(0);
        _gc[_count] = (byte)(gc != _lastGc ? 1 : 0);
        _lastGc = gc;
        _ts[_count++] = Stopwatch.GetTimestamp();
    }
}

// The resolved render-camera pose, captured as the render pipeline begins rendering the
// target camera (RenderPipelineManager.beginCameraRendering; OwlcatRenderPipeline raises
// it per camera). A struct copy per frame, no allocation.
internal struct CameraPose
{
    public int Frame;
    public Vector3 Position;
    public Quaternion Rotation;
    public float Fov, Near, Far, Aspect, OrthoSize;
    public bool Ortho;
    public Rect PixelRect;
    public int PixelWidth, PixelHeight;
    public Matrix4x4 Projection, WorldToCamera, NonJitteredProjection;

    public static CameraPose Of(Camera c) => new CameraPose
    {
        Frame = Time.frameCount,
        Position = c.transform.position,
        Rotation = c.transform.rotation,
        Fov = c.fieldOfView,
        Near = c.nearClipPlane,
        Far = c.farClipPlane,
        Aspect = c.aspect,
        Ortho = c.orthographic,
        OrthoSize = c.orthographicSize,
        PixelRect = c.pixelRect,
        PixelWidth = c.pixelWidth,
        PixelHeight = c.pixelHeight,
        Projection = c.projectionMatrix,
        NonJitteredProjection = c.nonJitteredProjectionMatrix,
        WorldToCamera = c.worldToCameraMatrix,
    };

    // Largest difference from another pose: position (m) and rotation (degrees), FOV.
    public float PositionDelta(in CameraPose o) => (Position - o.Position).magnitude;
    public float AngleDelta(in CameraPose o) => Quaternion.Angle(Rotation, o.Rotation);
}

internal static class PoseProbe
{
    public static Camera Target;
    public static CameraPose Last;
    public static bool HaveLast;
    // Stability tracking against a reference pose.
    public static CameraPose Reference;
    public static bool Tracking;
    public static float MaxPosDelta, MaxAngleDelta, MaxFovDelta;
    public static int TrackedFrames;
    public static int OtherCamerasThisFrame;

    public static void Install()
    {
        RenderPipelineManager.beginCameraRendering -= OnBegin;
        RenderPipelineManager.beginCameraRendering += OnBegin;
    }

    public static void Uninstall()
    {
        RenderPipelineManager.beginCameraRendering -= OnBegin;
        Target = null;
        HaveLast = false;
        Tracking = false;
    }

    public static void StartTracking(in CameraPose reference)
    {
        Reference = reference;
        MaxPosDelta = MaxAngleDelta = MaxFovDelta = 0;
        TrackedFrames = 0;
        Tracking = true;
    }

    private static void OnBegin(ScriptableRenderContext ctx, Camera cam)
    {
        if (cam != Target)
        {
            OtherCamerasThisFrame++;
            return;
        }
        Last = CameraPose.Of(cam);
        HaveLast = true;
        if (!Tracking) return;
        TrackedFrames++;
        var p = Last.PositionDelta(Reference);
        var a = Last.AngleDelta(Reference);
        var f = Math.Abs(Last.Fov - Reference.Fov);
        if (p > MaxPosDelta) MaxPosDelta = p;
        if (a > MaxAngleDelta) MaxAngleDelta = a;
        if (f > MaxFovDelta) MaxFovDelta = f;
    }
}
