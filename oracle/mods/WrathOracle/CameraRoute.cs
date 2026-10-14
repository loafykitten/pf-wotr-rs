using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using Kingmaker;
using Kingmaker.Blueprints;
using Kingmaker.QA.Arbiter;
using Kingmaker.UI;
using Kingmaker.Utility;
using Kingmaker.View;
using Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar;
using UnityEngine;
using UnityModManagerNet;

namespace WrathOracle;

// The matched workload's camera: the Caves_1 Arbiter instruction's eight authored points
// (docs/poc0/arbiter.md), driven through CameraRig's own immediate operations in
// Arbiter's order (scroll, then yaw, then zoom), with an explicit schedule. At each point
// it records the resolved render-camera pose and projection (captured as the pipeline
// begins rendering the game camera) and either a screenshot (shots) or every frame time
// over a fixed dwell (bench). The game keeps running (live profile, not paused).
internal static class CameraRoute
{
    internal struct Point
    {
        public string Name;
        public Vector3 Position;
        public float Yaw, Zoom;
    }

    public const string InstructionGuid = "0c9239f8cff8dd348a3a4251573d9fd6";
    public const string AreaGuid = "bad465e5404604346bdf71ef50b3299a";

    // From the shipped blueprint (QA/Arbiter/Scenario/Act_0_Prologue/Caves_1.jbp); used only
    // if the instruction can't be resolved in game, and compared with it when it can.
    internal static readonly Point[] AuthoredPoints =
    {
        new Point { Name = "Caves_1_1604385054612", Position = new Vector3(120.859131f, 39.99878f, 144.380127f), Yaw = 343.5062f },
        new Point { Name = "Caves_1_1604385062629", Position = new Vector3(89.7974854f, 39.15758f, 141.323242f), Yaw = 139.810257f },
        new Point { Name = "Caves_1_1604385069002", Position = new Vector3(42.9290924f, 34.6545143f, 153.390091f), Yaw = 77.8682861f },
        new Point { Name = "Caves_1_1604385076909", Position = new Vector3(51.7694359f, 41.88687f, 127.315735f), Yaw = 357.044678f },
        new Point { Name = "Caves_1_1604385085659", Position = new Vector3(112.482346f, 46.14825f, 102.853241f), Yaw = 336.52478f },
        new Point { Name = "Caves_1_1604385091407", Position = new Vector3(123.299934f, 45.61065f, 80.6090851f), Yaw = 342.1219f },
        new Point { Name = "Caves_1_1604385098289", Position = new Vector3(78.56158f, 39.10982f, 67.9544449f), Yaw = 144.222931f },
        new Point { Name = "Caves_1_1604385108701", Position = new Vector3(50.48398f, 38.9198341f, 59.285038f), Yaw = 115.383659f },
    };

    private sealed class PointResult
    {
        public Point Requested;
        public Vector3 RigPosition, RigTarget;
        public Quaternion RigRotation;
        public CameraPose Pose;
        public bool HavePose;
        public string Shot;
        public string ShotSha;
        public int ShotFrame;
        public double MoveT, CaptureT;
        public int MoveFrame;
        public float MaxPos, MaxAngle, MaxFov;
        public int Tracked;
        public float ZoomNormalized;
        public int FrameStart, FrameEnd; // FrameClock indices (bench)
        public string GameTime;
        public List<(string name, Vector3 lp, Quaternion lr, Vector3 ls)> Chain;
    }

    public static IEnumerator Run(Plan plan, int pass, string outDir, Action<string> fail)
    {
        var s = plan.Route;
        var game = Game.Instance;
        var area = game.CurrentlyLoadedArea;
        if (area == null || area.AssetGuid.ToString() != AreaGuid)
        {
            fail($"route: loaded area is {(area != null ? area.name : "none")}, expected Prologue_Caves_1");
            yield break;
        }
        var rig = game.UI.GetCameraRig();
        if (rig == null || rig.Camera == null)
        {
            fail("route: camera rig not ready");
            yield break;
        }

        // Resolve the instruction by GUID (bypasses Arbiter's missing name index).
        var points = AuthoredPoints;
        string source = "authored fallback", mismatch = null;
        try
        {
            var instr = ResourcesLibrary.TryGetBlueprint<BlueprintArbiterInstruction>(InstructionGuid);
            var test = instr?.GetTest<AreaCheckerComponent>();
            if (test != null)
            {
                var live = new List<Point>();
                foreach (var part in test.AreaParts.ElementList.OfType<AreaCheckerComponentPart>())
                    foreach (var p in part.Points)
                        live.Add(new Point { Name = p.name, Position = p.Position, Yaw = p.Rotation, Zoom = p.Zoom });
                if (live.Count > 0)
                {
                    points = live.ToArray();
                    source = $"instruction {InstructionGuid} ({instr.name}), area {test.Area.Get()?.name}";
                    if (points.Length != AuthoredPoints.Length ||
                        points.Where((p, i) => p.Position != AuthoredPoints[i].Position || p.Yaw != AuthoredPoints[i].Yaw || p.Zoom != AuthoredPoints[i].Zoom).Any())
                        mismatch = "live instruction differs from the authored copy";
                }
            }
        }
        catch (Exception e)
        {
            Health.Failed("resolve Arbiter instruction", e);
        }
        Main.Note($"route pass {pass}: {points.Length} points from {source}{(mismatch != null ? "; " + mismatch : "")}; mode {s.Mode}");

        // Take ownership of what we change, and restore exactly that afterwards.
        var canvases = new List<(GameObject go, bool active)>();
        var screenCanvases = new List<Canvas>();
        var fow = FogOfWarArea.Active;
        var fowWasEnabled = fow != null && fow.enabled;
        var guard = rig.FixCamera;
        var acquired = false;
        var results = new List<PointResult>();
        var cameraNames = new HashSet<string>();
        try
        {
            if (UnityModManager.UI.Instance != null && UnityModManager.UI.Instance.Opened) UnityModManager.UI.Instance.ToggleWindow(false);
            if (s.HideUi)
            {
                foreach (var c in Resources.FindObjectsOfTypeAll<StaticCanvas>().Select(c => c.gameObject)
                             .Concat(Resources.FindObjectsOfTypeAll<DynamicCanvas>().Select(c => c.gameObject)))
                {
                    if (!c.scene.IsValid()) continue; // prefab assets, not instances
                    canvases.Add((c, c.activeSelf));
                    c.SetActive(false);
                }
                // Other screen-space UI (e.g. the combat log lives on its own canvas):
                // disable every enabled root canvas that isn't world-space.
                foreach (var cv in Resources.FindObjectsOfTypeAll<Canvas>())
                {
                    if (cv == null || !cv.gameObject.scene.IsValid() || !cv.isRootCanvas || !cv.enabled) continue;
                    if (cv.renderMode == RenderMode.WorldSpace) continue;
                    cv.enabled = false;
                    screenCanvases.Add(cv);
                }
            }
            if (s.Fow == "off" && fow != null) fow.enabled = false;
            guard.Value = true;
            acquired = true;
            PoseProbe.Target = rig.Camera;
            PoseProbe.Install();
            if (s.Mode == "bench") FrameClock.Instance.Arm();
            yield return null;
            yield return null;

            for (var i = 0; i < points.Length; i++)
            {
                var pt = points[i];
                var r = new PointResult { Requested = pt };
                // Arbiter's MoveCameraToPoint order. ScrollToImmediately grounds the target
                // with a physics query along the *current* camera forward, so order matters.
                rig.ScrollToImmediately(pt.Position);
                rig.RotateToImmediately(pt.Yaw);
                rig.CameraZoom.ZoomToImmediate(pt.Zoom);
                r.MoveT = Time.realtimeSinceStartup;
                r.MoveFrame = Time.frameCount;
                r.FrameStart = s.Mode == "bench" ? FrameClock.Instance.Count : 0;
                Main.Note($"route pass {pass} point {i} ({pt.Name})");

                // Wait two frames for the move to render, then track pose stability.
                yield return null;
                yield return null;
                PoseProbe.StartTracking(PoseProbe.Last);
                var wait = s.Mode == "bench" ? s.Dwell : s.Settle;
                var until = r.MoveT + wait;
                while (Time.realtimeSinceStartup < until) yield return null;

                if (s.Mode == "shots")
                {
                    yield return new WaitForEndOfFrame();
                    var tex = ScreenCapture.CaptureScreenshotAsTexture();
                    r.ShotFrame = Time.frameCount;
                    r.Pose = PoseProbe.Last;
                    r.HavePose = PoseProbe.HaveLast && r.Pose.Frame == r.ShotFrame;
                    r.CaptureT = Time.realtimeSinceStartup;
                    try
                    {
                        var png = tex.EncodeToPNG();
                        r.Shot = Path.Combine("shots", $"pass{pass}-point{i}.png");
                        var full = Path.Combine(outDir, r.Shot);
                        Directory.CreateDirectory(Path.GetDirectoryName(full));
                        File.WriteAllBytes(full, png);
                        r.ShotSha = Json.Sha256(png);
                        Health.FileWritten(full);
                    }
                    catch (Exception e)
                    {
                        Health.Failed($"screenshot point {i}", e);
                    }
                    finally
                    {
                        UnityEngine.Object.Destroy(tex);
                    }
                }
                else
                {
                    r.Pose = PoseProbe.Last;
                    r.HavePose = PoseProbe.HaveLast;
                    r.CaptureT = Time.realtimeSinceStartup;
                    r.FrameEnd = FrameClock.Instance.Count;
                }
                PoseProbe.Tracking = false;
                r.MaxPos = PoseProbe.MaxPosDelta;
                r.MaxAngle = PoseProbe.MaxAngleDelta;
                r.MaxFov = PoseProbe.MaxFovDelta;
                r.Tracked = PoseProbe.TrackedFrames;
                r.RigPosition = rig.transform.position;
                r.RigRotation = rig.transform.rotation;
                r.RigTarget = rig.GetPosition();
                r.ZoomNormalized = rig.CameraZoom.CurrentNormalizePosition;
                r.GameTime = game.TimeController.GameTime.ToString();
                r.Chain = new List<(string, Vector3, Quaternion, Vector3)>();
                for (var t = rig.Camera.transform; t != null; t = t.parent)
                {
                    r.Chain.Add((t.name, t.localPosition, t.localRotation, t.localScale));
                    if (t == rig.transform) break;
                }
                results.Add(r);
                Health.Written();
            }
            if (s.Mode == "bench") FrameClock.Instance.Disarm();
        }
        finally
        {
            if (s.Mode == "bench") FrameClock.Instance.Disarm();
            PoseProbe.Uninstall();
            if (acquired && guard.GuardCount > 0) guard.Value = false; // release only our own acquisition
            if (fow != null && s.Fow == "off") fow.enabled = fowWasEnabled;
            foreach (var (go, active) in canvases)
                if (go != null) go.SetActive(active);
            foreach (var cv in screenCanvases)
                if (cv != null) cv.enabled = true;
        }

        Write(plan, pass, outDir, points, source, mismatch, results, rig, fowWasEnabled);
        Main.Note($"route pass {pass} done");
    }

    private static void Write(Plan plan, int pass, string outDir, Point[] points, string source, string mismatch,
        List<PointResult> results, CameraRig rig, bool fowWasEnabled)
    {
        var s = plan.Route;
        using var j = new Json(Path.Combine(outDir, $"route-pass{pass}.json"), indent: true);
        j.Obj();
        j.Prop("schema", "wrath-camera-replay-v1");
        j.Obj("coordinates")
            .Prop("space", "Unity world (left-handed)")
            .Prop("up", "+Y")
            .Prop("camera_forward", "+Z of the camera transform; Unity view space looks down -Z (worldToCameraMatrix)")
            .Prop("quaternion_order", "xyzw")
            .Prop("matrix_layout", "row-major: m[row][column] = matrix[row, column]")
            .Prop("units", "metres; FOV vertical degrees")
            .End();
        j.Prop("area_guid", AreaGuid);
        j.Prop("instruction_guid", InstructionGuid);
        j.Prop("points_source", source);
        j.Prop("points_mismatch", mismatch);
        j.Prop("save", plan.Save);
        j.Prop("pass", pass);
        j.Obj("timing")
            .Prop("policy", s.Mode == "bench" ? "fixed-wall-clock-dwell" : "settle-then-capture")
            .Prop("transition", "step (teleport; no interpolation)")
            .Prop("move_order", "ScrollToImmediately(position), RotateToImmediately(yaw), CameraZoom.ZoomToImmediate(zoom)")
            .Prop("settle_s", s.Settle)
            .Prop("dwell_s", s.Dwell)
            .Prop("warmup_frames_reported_separately", s.WarmupFrames)
            .Prop("simulation", "live (game not paused)")
            .End();
        j.Obj("profile")
            .Prop("ui_hidden", s.HideUi ? "StaticCanvas/DynamicCanvas objects inactive and every enabled screen-space root Canvas disabled" : "no")
            .Prop("fog_of_war", s.Fow == "off" ? "FogOfWarArea disabled during the route" : "as loaded")
            .Prop("fog_of_war_area_present", FogOfWarArea.Active != null)
            .Prop("fog_of_war_was_enabled", fowWasEnabled)
            .Prop("units_visible", true)
            .Prop("vsync_count", QualitySettings.vSyncCount)
            .Prop("target_frame_rate", Application.targetFrameRate)
            .Prop("screen", $"{Screen.width}x{Screen.height}")
            .Prop("time_of_day", Game.Instance.TimeOfDay.ToString())
            .Prop("weather", Weather())
            .End();
        j.Obj("camera_static")
            .Prop("fov_min", rig.CameraZoom.FovMin)
            .Prop("fov_max", rig.CameraZoom.FovMax)
            .Prop("camera_name", rig.Camera.name)
            .Prop("rig_new_behaviour", rig.NewBehaviour)
            .Prop("rig_mode", rig.Mode.ToString())
            .Prop("allow_hdr", rig.Camera.allowHDR)
            .Prop("allow_msaa", rig.Camera.allowMSAA)
            .Prop("culling_mask", rig.Camera.cullingMask)
            .Prop("depth", rig.Camera.depth)
            .End();
        j.Arr("points");
        for (var i = 0; i < results.Count; i++)
        {
            var r = results[i];
            j.Obj();
            j.Prop("sample", i);
            j.Prop("name", r.Requested.Name);
            j.Obj("requested").Prop("rig_position", r.Requested.Position).Prop("yaw_deg", r.Requested.Yaw).Prop("zoom_raw", r.Requested.Zoom).End();
            j.Obj("rig").Prop("position", r.RigPosition).Prop("rotation", r.RigRotation).Prop("target", r.RigTarget).Prop("zoom_normalized", r.ZoomNormalized).End();
            j.Arr("camera_chain"); // camera first, up to the rig
            foreach (var c in r.Chain)
                j.Obj().Prop("name", c.name).Prop("local_position", c.lp).Prop("local_rotation", c.lr).Prop("local_scale", c.ls).End();
            j.EndArr();
            if (r.HavePose)
            {
                var p = r.Pose;
                j.Obj("camera");
                j.Prop("frame", p.Frame);
                j.Prop("position", p.Position);
                j.Prop("rotation", p.Rotation);
                j.Prop("fov_vertical_deg", p.Fov);
                j.Prop("near", p.Near);
                j.Prop("far", p.Far);
                j.Prop("aspect", p.Aspect);
                j.Prop("orthographic", p.Ortho);
                j.Prop("orthographic_size", p.OrthoSize);
                j.Prop("pixel_rect", p.PixelRect);
                j.Prop("pixel_width", p.PixelWidth);
                j.Prop("pixel_height", p.PixelHeight);
                j.Prop("world_to_camera", p.WorldToCamera);
                j.Prop("projection", p.Projection);
                j.Prop("non_jittered_projection", p.NonJitteredProjection);
                j.Prop("gpu_projection_render_into_texture", GL.GetGPUProjectionMatrix(p.Projection, true));
                j.End();
            }
            else
            {
                j.Null("camera");
            }
            j.Obj("stability").Prop("frames_tracked", r.Tracked).Prop("max_position_delta_m", r.MaxPos).Prop("max_angle_delta_deg", r.MaxAngle).Prop("max_fov_delta_deg", r.MaxFov).End();
            j.Prop("move_frame", r.MoveFrame);
            j.Prop("move_realtime_s", r.MoveT);
            j.Prop("capture_realtime_s", r.CaptureT);
            j.Prop("game_time", r.GameTime);
            if (r.Shot != null)
            {
                j.Prop("screenshot", r.Shot);
                j.Prop("screenshot_sha256", r.ShotSha);
                j.Prop("screenshot_frame", r.ShotFrame);
                j.Prop("screenshot_method", "ScreenCapture.CaptureScreenshotAsTexture after WaitForEndOfFrame (final back buffer, UI canvases hidden)");
            }
            if (s.Mode == "bench") WriteFrames(j, r.FrameStart, r.FrameEnd, s.WarmupFrames);
            j.End();
        }
        j.EndArr();
        if (s.Mode == "bench" && results.Count > 0)
        {
            j.Key("pass_frames");
            WriteStats(j, results[0].FrameStart, results[results.Count - 1].FrameEnd);
            j.Prop("frame_clock_dropped", FrameClock.Instance.Dropped);
            if (FrameClock.Instance.Dropped > 0) Health.Dropped(FrameClock.Instance.Dropped);
        }
        Health.Write(j);
        j.End();
    }

    private static string Weather()
    {
        try
        {
            var w = Owlcat.Runtime.Visual.Effects.WeatherSystem.VFXWeatherSystem.Instance;
            return w != null ? w.SystemForWeatherController?.Inclemency.ToString() : "no VFXWeatherSystem";
        }
        catch (Exception e)
        {
            return "unknown: " + e.Message;
        }
    }

    // FrameClock interval k is the frame between timestamps k-1 and k. A point owns the
    // half-open range [start, end): start is the clock count when the camera moved, so
    // interval `start` is the frame that rendered the move, and the frame of the next move
    // belongs to the next point. Raw values as base64 uint32 microseconds, plus stats with
    // and without the first `warmup` frames.
    private static (int lo, int hi) Clamp(int start, int end) =>
        (Math.Max(1, start), Math.Min(end, FrameClock.Instance.Count));

    private static void WriteFrames(Json j, int start, int end, int warmup)
    {
        j.Prop("frame_range", $"[{start},{end})");
        j.Key("frames");
        WriteStats(j, start, end);
        j.Key("frames_after_warmup");
        WriteStats(j, Math.Min(start + warmup, end), end);
        var (lo, hi) = Clamp(start, end);
        var n = Math.Max(0, hi - lo);
        var bytes = new byte[n * 4];
        for (var k = 0; k < n; k++)
        {
            var us = (uint)Math.Min(uint.MaxValue, Math.Round(FrameClock.Instance.IntervalMs(lo + k) * 1000.0));
            Buffer.BlockCopy(BitConverter.GetBytes(us), 0, bytes, k * 4, 4);
        }
        j.Prop("frame_us_u32_le_base64", Convert.ToBase64String(bytes));
    }

    private static void WriteStats(Json j, int start, int end)
    {
        var list = new List<double>();
        var gc = 0;
        var (lo, hi) = Clamp(start, end);
        for (var k = lo; k < hi; k++)
        {
            list.Add(FrameClock.Instance.IntervalMs(k));
            if (FrameClock.Instance.GcAt(k)) gc++;
        }
        j.Obj();
        j.Prop("count", list.Count);
        if (list.Count > 0)
        {
            var sum = list.Sum();
            var sorted = list.OrderBy(x => x).ToList();
            double P(double q) => sorted[Math.Max(0, Math.Min(sorted.Count - 1, (int)Math.Ceiling(q * sorted.Count) - 1))];
            j.Prop("seconds", sum / 1000.0);
            j.Prop("fps", list.Count / (sum / 1000.0));
            j.Prop("mean_ms", sum / list.Count);
            j.Prop("p50_ms", P(0.50));
            j.Prop("p95_ms", P(0.95));
            j.Prop("p99_ms", P(0.99));
            j.Prop("max_ms", sorted[sorted.Count - 1]);
            j.Prop("over_33ms", list.Count(x => x >= 33.0));
            j.Prop("over_50ms", list.Count(x => x >= 50.0));
            j.Prop("gc_frames", gc);
        }
        j.End();
    }
}
