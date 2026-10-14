using System;
using System.Collections;
using UnityEngine;

namespace WrathOracle;

// The phase of the fixed-step (physics) clock against the frame clock, pinned in turn-based replays.
//
// With fixed 1/60 s frames and Unity's 0.02 s fixed step, five frames in six run one physics step and
// one runs none; which one is set by phi = Time.time - Time.fixedTime, and phi at a window's start
// depends on how many frames the reload took (the loading screen runs frames on the fixed delta), so
// every run has its own. It matters: Physics.autoSyncTransforms is off, so the entity-bounds colliders
// (EntityBoundsController moves them to the units every tick) reach the physics scene only at a step,
// and the overlap queries on that scene (UnitVisionRangeController's vision candidates, which decide
// when a unit is first sampled for line of sight; the area-of-effect unit searches) see positions one
// tick older on frames without one [M: poc5-tb-40, a vision candidate's first line-of-sight sample, and
// the 0.5 m sight cache steps from it, decide the tick an enemy group notices the PC].
//
// A recording observes: the trace header has phi at the window's start and the number of frames from
// the reload's end (the anchor) to it. A turn-based replay reproduces it: at its own anchor it knows the
// frame its window will open on (the same number of frames later), and in the frames before that it
// steers phi to the value whose normal evolution reaches the recording's phi on that frame, by giving
// at most a few pre-window frames another fixed step (one step of another length, or none), then
// restoring the native step. Nothing changes inside the window. The replay's header has its phi at the
// window's start, and a replay whose phi differs from the recording's is invalid.
internal static class PhysicsPhase
{
    private static int _anchorFrame = -1;
    private static double? _target;      // the recording's phi at its window's start (replay)
    private static int _targetFrames;    // the recording's frames from its anchor to its window's start
    private static float _native;
    private static bool _steering, _owned;
    public static int SteerFrames;
    public static string SteerNote;

    public static double Phase => Time.timeAsDouble - Time.fixedTimeAsDouble;
    public static int FramesSinceAnchor => _anchorFrame < 0 ? -1 : Time.frameCount - _anchorFrame;

    // Driver "prepare" before a turn-based replay: the recording's values (null: nothing to pin).
    public static void SetTarget(double? phase, int frames)
    {
        Stop();
        _target = phase;
        _targetFrames = frames;
        _anchorFrame = -1;
        SteerFrames = 0;
        SteerNote = phase.HasValue ? "armed" : "not armed";
    }

    // The reload has finished (Runner.Load): the anchor. A replay with a target starts steering.
    public static void Anchor(MonoBehaviour host)
    {
        _anchorFrame = Time.frameCount;
        if (!_target.HasValue || host == null) return;
        _native = Time.fixedDeltaTime;
        _steering = true;
        host.StartCoroutine(Steer(_anchorFrame + _targetFrames, _target.Value));
    }

    private static double Wrap(double x, double d)
    {
        var r = x % d;
        return r < 0 ? r + d : r;
    }

    private static double Distance(double a, double b, double d)
    {
        var x = Wrap(a - b, d);
        return Math.Min(x, d - x);
    }

    // Runs after each frame's fixed steps (a coroutine resumes after Update); a fixedDeltaTime set here
    // applies to the next frame's steps. Next frame: phi' = phi + h - n * step, n the steps it runs. To land
    // on t: one step of length phi + h - t if that is one step (t < (phi + h) / 2), else no step this frame
    // (phi grows by h) and again next frame.
    private static IEnumerator Steer(int openFrame, double target)
    {
        var d = (double)_native;
        while (_steering)
        {
            var left = openFrame - Time.frameCount;
            if (left <= 0) break;
            // Time.time advances by captureDeltaTime * timeScale; the targets assume the scale stays as it is
            // (it is 1 in these frames; a different one is noted, and the check at the window's start decides).
            var h = (double)Time.captureDeltaTime * Time.timeScale;
            if (Time.timeScale != 1f) _scaleNote = $" (time scale {Time.timeScale:R} while steering)";
            var phi = Phase;
            var want = Wrap(target - left * h, d);
            if (Distance(phi, want, d) < 1e-7 || left < 2)
            {
                // On course (or no time left): the native step from here on.
                if (_owned) { Time.fixedDeltaTime = _native; _owned = false; }
            }
            else
            {
                var next = Wrap(want + h, d);
                var s = phi + h;
                Time.fixedDeltaTime = (float)(next < s / 2 ? s - next : s + d / 4);
                _owned = true;
                SteerFrames++;
            }
            yield return null;
        }
        Stop();
    }

    // Undoes the steering (idempotent); the window's start and cleanup call it.
    public static void Stop()
    {
        _steering = false;
        if (_owned) { Time.fixedDeltaTime = _native; _owned = false; }
    }

    private static string _scaleNote = "";

    // The window starts (Fixture.Begin, after the tape, before the header): a replay's steering ends here.
    // A turn-based replay must have taken the recording's physics: the same settings and phase.
    public static void WindowOpen(bool turnBasedReplay)
    {
        Stop();
        try
        {
            if (!turnBasedReplay) { SteerNote = _target.HasValue ? "armed, not a turn-based replay" : "not armed"; return; }
            var rec = Sched.RecPhysics;
            if (rec == null) { Unpinned = "physics not reproducible: the recording has no physics settings (re-record)"; SteerNote = "no recorded settings"; return; }
            if (rec.Value.sync != Physics.autoSyncTransforms || rec.Value.sim != Physics.autoSimulation || Math.Abs(rec.Value.step - Time.fixedDeltaTime) > 1e-9)
                Unpinned = $"physics settings differ: recording sync={rec.Value.sync} sim={rec.Value.sim} step={rec.Value.step:R}, replay sync={Physics.autoSyncTransforms} sim={Physics.autoSimulation} step={(double)Time.fixedDeltaTime:R}";
            if (!_target.HasValue) { Unpinned = Unpinned ?? "physics phase not reproduced: no target (the recording has no phase, or the replay had no prepare and reload)"; SteerNote = "not armed"; return; }
            var d = (double)Time.fixedDeltaTime;
            var off = Distance(Phase, _target.Value, d);
            var frames = FramesSinceAnchor;
            SteerNote = $"target {_target.Value:R}, phase {Phase:R}, off {off:G3}, {SteerFrames} steered frames, frames from anchor {frames} (recording {_targetFrames}){_scaleNote}";
            if (off > 1e-6 || frames != _targetFrames)
                Unpinned = Unpinned ?? $"physics phase not reproduced: {SteerNote}";
        }
        finally
        {
            _target = null;
            _scaleNote = "";
        }
    }

    // Set by WindowOpen when the replay could not take the recording's phase (Fixture invalidates it).
    public static string Unpinned;
}
