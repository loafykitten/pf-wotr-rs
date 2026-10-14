using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Clicks;
using Kingmaker.Controllers.Clicks.Handlers;
using Kingmaker.EntitySystem.Entities;
using Rewired;
using UnityEngine;

namespace WrathOracle;

// Player input in fixture windows. Mouse, keyboard and gamepad are the fixture's to define, in a
// recording and its replay alike: wherever the hardware cursor rests, a key someone touches or a
// window losing focus must not reach a fixture window. So from a fixture's "prepare" (before its
// reload, so the window opens on input-free pointer, camera and UI state) to the window's end, the
// game sees neutral input at the source, and its own pointer, camera and UI logic keeps running on it:
//   * every call site in the game's assemblies (Assembly-CSharp, firstpass, Owlcat, UnityEngine.UI,
//     TextMeshPro, ...) that reads UnityEngine.Input is transpiled to a replacement with the same
//     signature (the getters are native calls Harmony can't wrap; method groups such as
//     Keys.Any(Input.GetKey) are ldftn references and are replaced too). While the pin is active the
//     pointer rests at NeutralPointer, far off screen (not near an edge, so no edge scrolling; no UI
//     element under it, so no hover or tooltip), no button or key is held, pressed or released, axes
//     (scroll wheel included) are zero, no text is typed, there are no touches. Outside the pin the
//     replacements return the hardware's values;
//   * Rewired (gamepads, and the native keyboard and mouse Rewired reads on its own): every controller
//     is disabled for the pin and the ones this disabled are re-enabled at its end;
//   * the window's focus: Kingmaker.Runner.OnApplicationFocus, which tells the game (KeyboardAccess
//     fires its key-up bindings on focus loss, AutoPause pauses) is held back and delivered at the end;
//   * the pointer's world pick (PointerController.SelectClickObject, a camera ray against the scene):
//     the pointer is over nothing, unless the fixture points at a unit (Hover, TurnBased.IssueAttack),
//     in which case it is over that unit's view, the way the game's own pick resolves a unit's collider,
//     with the click handler the game's priority loop gives it.
// At the window's start the pointer's last world position (what a turn's terrain preview paths go to
// while the pointer is over nothing) is the main character's position.
internal static class InputPin
{
    private static volatile bool _active;
    public static bool Active => _active;

    // Far off screen in both directions: |x|, |y| are far from 0 and from the screen size (CameraRig's
    // edge scroll), not in (-50, 0) (its bottom band), and no rect contains it.
    public static readonly Vector3 NeutralPointer = new Vector3(100000f, 100000f, 0f);

    // The unit the fixture's pointer is over (null: over nothing).
    public static UnitEntityData Hover;

    public static long Reads, Picks, FocusHeld;
    public static int PatchedCallers, FailedCallers, CallersFound;
    public static string ScanNote, BeginNote;

    // ---- replacements for UnityEngine.Input (same signatures) ----

    public static Vector3 MousePosition() { if (!_active) return Input.mousePosition; Reads++; return NeutralPointer; }
    public static Vector2 MouseScrollDelta() { if (!_active) return Input.mouseScrollDelta; Reads++; return Vector2.zero; }
    public static bool GetMouseButton(int b) { if (!_active) return Input.GetMouseButton(b); Reads++; return false; }
    public static bool GetMouseButtonDown(int b) { if (!_active) return Input.GetMouseButtonDown(b); Reads++; return false; }
    public static bool GetMouseButtonUp(int b) { if (!_active) return Input.GetMouseButtonUp(b); Reads++; return false; }
    public static float GetAxis(string n) { if (!_active) return Input.GetAxis(n); Reads++; return 0f; }
    public static float GetAxisRaw(string n) { if (!_active) return Input.GetAxisRaw(n); Reads++; return 0f; }
    public static bool GetButton(string n) { if (!_active) return Input.GetButton(n); Reads++; return false; }
    public static bool GetButtonDown(string n) { if (!_active) return Input.GetButtonDown(n); Reads++; return false; }
    public static bool GetButtonUp(string n) { if (!_active) return Input.GetButtonUp(n); Reads++; return false; }
    public static bool GetKeyCode(KeyCode k) { if (!_active) return Input.GetKey(k); Reads++; return false; }
    public static bool GetKeyName(string n) { if (!_active) return Input.GetKey(n); Reads++; return false; }
    public static bool GetKeyDownCode(KeyCode k) { if (!_active) return Input.GetKeyDown(k); Reads++; return false; }
    public static bool GetKeyDownName(string n) { if (!_active) return Input.GetKeyDown(n); Reads++; return false; }
    public static bool GetKeyUpCode(KeyCode k) { if (!_active) return Input.GetKeyUp(k); Reads++; return false; }
    public static bool GetKeyUpName(string n) { if (!_active) return Input.GetKeyUp(n); Reads++; return false; }
    public static bool AnyKey() { if (!_active) return Input.anyKey; Reads++; return false; }
    public static bool AnyKeyDown() { if (!_active) return Input.anyKeyDown; Reads++; return false; }
    public static string InputString() { if (!_active) return Input.inputString; Reads++; return ""; }
    public static string CompositionString() { if (!_active) return Input.compositionString; Reads++; return ""; }
    public static int TouchCount() { if (!_active) return Input.touchCount; Reads++; return 0; }
    public static Touch[] Touches() { if (!_active) return Input.touches; Reads++; return new Touch[0]; }
    public static Touch GetTouch(int i) { if (!_active) return Input.GetTouch(i); Reads++; return default; }
    public static bool MousePresent() { if (!_active) return Input.mousePresent; Reads++; return true; }
    public static Vector3 Acceleration() { if (!_active) return Input.acceleration; Reads++; return Vector3.zero; }
    public static int AccelerationEventCount() { if (!_active) return Input.accelerationEventCount; Reads++; return 0; }
    public static string[] GetJoystickNames() { if (!_active) return Input.GetJoystickNames(); Reads++; return new string[0]; }
    // The IMGUI event queue that UI text fields (InputField, TMP_InputField OnUpdateSelected) drain for keys.
    public static bool PopEvent(Event outEvent) { if (!_active) return Event.PopEvent(outEvent); Reads++; return false; }

    // UnityEngine.Input member (name, parameter types) -> replacement.
    private static readonly (Type type, string member, Type[] args, string repl)[] Members =
    {
        (typeof(Event), "PopEvent", new[] { typeof(Event) }, nameof(PopEvent)),
        (typeof(Input), "get_mousePosition", Type.EmptyTypes, nameof(MousePosition)),
        (typeof(Input), "get_mouseScrollDelta", Type.EmptyTypes, nameof(MouseScrollDelta)),
        (typeof(Input), "GetMouseButton", new[] { typeof(int) }, nameof(GetMouseButton)),
        (typeof(Input), "GetMouseButtonDown", new[] { typeof(int) }, nameof(GetMouseButtonDown)),
        (typeof(Input), "GetMouseButtonUp", new[] { typeof(int) }, nameof(GetMouseButtonUp)),
        (typeof(Input), "GetAxis", new[] { typeof(string) }, nameof(GetAxis)),
        (typeof(Input), "GetAxisRaw", new[] { typeof(string) }, nameof(GetAxisRaw)),
        (typeof(Input), "GetButton", new[] { typeof(string) }, nameof(GetButton)),
        (typeof(Input), "GetButtonDown", new[] { typeof(string) }, nameof(GetButtonDown)),
        (typeof(Input), "GetButtonUp", new[] { typeof(string) }, nameof(GetButtonUp)),
        (typeof(Input), "GetKey", new[] { typeof(KeyCode) }, nameof(GetKeyCode)),
        (typeof(Input), "GetKey", new[] { typeof(string) }, nameof(GetKeyName)),
        (typeof(Input), "GetKeyDown", new[] { typeof(KeyCode) }, nameof(GetKeyDownCode)),
        (typeof(Input), "GetKeyDown", new[] { typeof(string) }, nameof(GetKeyDownName)),
        (typeof(Input), "GetKeyUp", new[] { typeof(KeyCode) }, nameof(GetKeyUpCode)),
        (typeof(Input), "GetKeyUp", new[] { typeof(string) }, nameof(GetKeyUpName)),
        (typeof(Input), "get_anyKey", Type.EmptyTypes, nameof(AnyKey)),
        (typeof(Input), "get_anyKeyDown", Type.EmptyTypes, nameof(AnyKeyDown)),
        (typeof(Input), "get_inputString", Type.EmptyTypes, nameof(InputString)),
        (typeof(Input), "get_compositionString", Type.EmptyTypes, nameof(CompositionString)),
        (typeof(Input), "get_touchCount", Type.EmptyTypes, nameof(TouchCount)),
        (typeof(Input), "get_touches", Type.EmptyTypes, nameof(Touches)),
        (typeof(Input), "GetTouch", new[] { typeof(int) }, nameof(GetTouch)),
        (typeof(Input), "get_mousePresent", Type.EmptyTypes, nameof(MousePresent)),
        (typeof(Input), "get_acceleration", Type.EmptyTypes, nameof(Acceleration)),
        (typeof(Input), "get_accelerationEventCount", Type.EmptyTypes, nameof(AccelerationEventCount)),
        (typeof(Input), "GetJoystickNames", Type.EmptyTypes, nameof(GetJoystickNames)),
    };

    // By module and metadata token.
    private static readonly Dictionary<(Module, int), MethodInfo> Map = new Dictionary<(Module, int), MethodInfo>();

    private static bool IsReplaced(MethodBase m) => m != null && Map.ContainsKey((m.Module, m.MetadataToken));

    public static IEnumerable<CodeInstruction> Transpiler(IEnumerable<CodeInstruction> ins)
    {
        foreach (var i in ins)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt || i.opcode == OpCodes.Ldftn) && i.operand is MethodBase mb && IsReplaced(mb))
            {
                if (i.opcode == OpCodes.Callvirt) i.opcode = OpCodes.Call;
                i.operand = Map[(mb.Module, mb.MetadataToken)];
            }
            yield return i;
        }
    }

    // ---- the pin ----

    private static readonly List<Controller> Disabled = new List<Controller>();
    private static bool _connectSubscribed;

    // From a fixture's "prepare" (Driver) or its start (Fixture.Begin, if it had no prepare). Idempotent.
    public static void Begin(string why)
    {
        if (_active || !Fidelity.Installed) return; // the replacements exist only with the fidelity hooks
        _active = true; // first: whatever fails below, End undoes what was done
        Reads = Picks = FocusHeld = GuiEventsUsed = 0;
        Hover = null;
        lock (Held) Held.Clear();
        _rewiredDone = false;
        ReloadedUnderPin = false;
        var notes = new List<string> { why };
        TryRewired(notes);
        try { ResetPress(notes); }
        catch (Exception e) { Health.Failed("input pin: pointer reset", e); }
        ResetUi(notes);
        ResetCamera(notes);
        BeginNote = string.Join("; ", notes);
        Main.Note("input pin on: " + BeginNote);
    }

    // After the window's trace is closed (Fixture.End's last step), or cleanup. Idempotent; undoes exactly
    // what Begin changed, then gives the game the focus and controller changes it held back, in order.
    public static void End()
    {
        if (!_active) return;
        Hover = null;
        try { ResetPress(null); }
        catch (Exception e) { Health.Failed("input pin: pointer reset at end", e); }
        _active = false;
        try { RestoreRewired(); }
        catch (Exception e) { Health.Failed("input pin: rewired restore", e); }
        try { DeliverHeld(); }
        catch (Exception e) { Health.Failed("input pin: held delivery", e); }
        Main.Note($"input pin off: {Reads} neutral reads, {Picks} pointer picks, {FocusHeld} focus/controller changes held, {GuiEventsUsed} IMGUI input events used");
    }

    // Rewired has to be up (it is long before a save loads); a pin that couldn't take it fails the fixture.
    private static bool _rewiredDone;

    private static void TryRewired(List<string> notes)
    {
        if (_rewiredDone) return;
        try
        {
            if (!ReInput.isReady) { notes?.Add("rewired not ready"); return; }
            if (!_connectSubscribed)
            {
                ReInput.ControllerConnectedEvent += OnConnected; // first: one connecting while we enumerate is caught
                _connectSubscribed = true;
            }
            var names = new List<string>();
            foreach (var c in ReInput.controllers.Controllers.ToList())
            {
                if (c == null || !c.enabled) continue;
                c.enabled = false;
                lock (Disabled) Disabled.Add(c);
                names.Add(c.type + ":" + c.name);
            }
            _rewiredDone = true;
            notes?.Add($"rewired controllers disabled: {names.Count} ({string.Join(", ", names)})");
        }
        catch (Exception e) { Health.Failed("input pin: rewired", e); }
    }

    private static void OnConnected(ControllerStatusChangedEventArgs e)
    {
        try
        {
            var c = e?.controller;
            if (!_active || c == null || !c.enabled) return;
            c.enabled = false;
            lock (Disabled) Disabled.Add(c);
            Sched.Obs("input_controller_connected", j => j.Prop("name", c.type + ":" + c.name));
        }
        catch (Exception x) { Health.Failed("input pin: controller connected", x); }
    }

    private static void RestoreRewired()
    {
        if (_connectSubscribed)
        {
            _connectSubscribed = false;
            try { ReInput.ControllerConnectedEvent -= OnConnected; } catch (Exception) { /* Rewired shut down */ }
        }
        List<Controller> list;
        lock (Disabled) { list = new List<Controller>(Disabled); Disabled.Clear(); }
        foreach (var c in list)
        {
            try { c.enabled = true; }
            catch (Exception e) { Health.Log("input pin: re-enable " + c?.name, e); }
        }
    }

    // ---- the game's pointer ----

    private static readonly AccessTools.FieldRef<PointerController, bool> MouseDown = AccessTools.FieldRefAccess<PointerController, bool>("m_MouseDown");
    private static readonly AccessTools.FieldRef<PointerController, bool> MouseDrag = AccessTools.FieldRefAccess<PointerController, bool>("m_MouseDrag");
    private static readonly AccessTools.FieldRef<PointerController, int> DragFrames = AccessTools.FieldRefAccess<PointerController, int>("m_DragFrames");
    private static readonly AccessTools.FieldRef<PointerController, GameObject> MouseDownOn = AccessTools.FieldRefAccess<PointerController, GameObject>("m_MouseDownOn");
    private static readonly AccessTools.FieldRef<PointerController, IClickEventHandler> MouseDownHandler = AccessTools.FieldRefAccess<PointerController, IClickEventHandler>("m_MouseDownHandler");
    private static readonly AccessTools.FieldRef<PointerController, IClickEventHandler> SimulateHandler = AccessTools.FieldRefAccess<PointerController, IClickEventHandler>("m_SimulateClickHandler");
    private static readonly AccessTools.FieldRef<PointerController, bool> PadConfirm = AccessTools.FieldRefAccess<PointerController, bool>("m_GamePadConfirm");
    private static readonly AccessTools.FieldRef<PointerController, bool> PadDecline = AccessTools.FieldRefAccess<PointerController, bool>("m_GamePadDecline");
    private static readonly AccessTools.FieldRef<PointerController, Vector3> SimPos = AccessTools.FieldRefAccess<PointerController, Vector3>("m_WorldPositionForSimulation");
    private static readonly AccessTools.FieldRef<PointerController, IClickEventHandler[]> Handlers = AccessTools.FieldRefAccess<PointerController, IClickEventHandler[]>("m_ClickHandlers");
    private static readonly MethodInfo SetOvertip = AccessTools.PropertySetter(typeof(PointerController), nameof(PointerController.OvertipObject));
    private static readonly MethodInfo SetWorldPosition = AccessTools.PropertySetter(typeof(PointerController), nameof(PointerController.WorldPosition));

    private static IEnumerable<PointerController> Pointers()
    {
        var g = Game.Instance;
        if (g == null) yield break;
        var a = g.DefaultPointerController;
        if (a != null) yield return a;
        var b = g.ClickEventsController;
        if (b != null && !ReferenceEquals(a, b)) yield return b;
    }

    // A press or drag in progress (a button held when the pin started) is cancelled, not completed: under
    // neutral input the next Tick would see the button released and click. The handler the last hover
    // selected is cleared; Tick and UpdateSelectedClickHandler set it again from the pinned pointer.
    private static void ResetPress(List<string> notes)
    {
        foreach (var pc in Pointers())
        {
            var state = $"down={MouseDown(pc)} drag={MouseDrag(pc)} mode={pc.Mode} on={(pc.PointerOn != null ? pc.PointerOn.name : "-")}";
            if (MouseDrag(pc)) Game.Instance.UI?.MultiSelection?.Cancel();
            MouseDown(pc) = false;
            MouseDrag(pc) = false;
            DragFrames(pc) = 0;
            MouseDownOn(pc) = null;
            MouseDownHandler(pc) = null;
            SimulateHandler(pc) = null;
            PadConfirm(pc) = false;
            PadDecline(pc) = false;
            if (pc.Mode != PointerMode.Default) pc.ClearPointerMode();
            notes?.Add("pointer " + state);
        }
    }

    // A fixture window must open on a reload made under the pin (Driver "prepare", then "reload"): the load
    // puts the camera at the save's position (and rotation) and rebuilds the pointer's hover, so what the
    // hardware did before the pin can't reach the window. Runner.Load notes the reload; Fixture.Begin checks.
    public static bool ReloadedUnderPin { get; private set; }
    public static void NoteReload() { if (_active) ReloadedUnderPin = true; }

    // The camera at the window's start (trace header; a turn-based replay must have the recording's):
    // rig position x, y, z, rig yaw, zoom (smoothed), field of view.
    public static double[] CameraState()
    {
        var rig = Game.Instance?.UI?.GetCameraRig();
        if (rig == null) return null;
        var p = rig.transform.position;
        var zoom = rig.CameraZoom;
        var cam = rig.Camera;
        return new double[] { p.x, p.y, p.z, rig.transform.eulerAngles.y, zoom != null ? SmoothScroll(zoom) : -1f, cam != null ? cam.fieldOfView : -1f };
    }

    // A camera gesture in progress (the middle button held to rotate, or Alt + middle button to drag) keeps
    // its latch and base point until a button-up that neutral input never gives; it is cancelled as the
    // game's own button-up does (latch off, base point dropped, cursor back), queued camera input is dropped,
    // and a rotation tween the player's input started is stopped where it is. (A prepared fixture's reload
    // then puts the rig at the save's position and rotation anyway; this matters for a window opened
    // without one.)
    private static readonly System.Reflection.FieldInfo RigRotationByMouse = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_RotationByMouse");
    private static readonly System.Reflection.FieldInfo RigBaseMousePoint = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_BaseMousePoint");
    private static readonly System.Reflection.FieldInfo RigRotateDistance = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_RotateDistance");
    private static readonly System.Reflection.FieldInfo RigRotateDistance2D = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_RotateDistance2D");
    private static readonly System.Reflection.FieldInfo RigScrollBy2D = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_ScrollBy2D");
    private static readonly System.Reflection.FieldInfo RigRotationByKeyboard = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_RotationByKeyboard");
    private static readonly System.Reflection.FieldInfo RigRotateOffset = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_RotateOffset");
    private static readonly System.Reflection.FieldInfo RigScrollOffset = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_ScrollOffset");
    private static readonly System.Reflection.FieldInfo RigScrollOffsetVertical = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_ScrollOffsetVertical");
    private static readonly System.Reflection.FieldInfo RigBankRotateOffset = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_BankRotateOffset");

    private static readonly AccessTools.FieldRef<Kingmaker.View.CameraZoom, Coroutine> ZoomRoutine = AccessTools.FieldRefAccess<Kingmaker.View.CameraZoom, Coroutine>("m_ZoomRoutine");
    private static readonly AccessTools.FieldRef<Kingmaker.View.CameraZoom, float> PlayerScroll = AccessTools.FieldRefAccess<Kingmaker.View.CameraZoom, float>("m_PlayerScrollPosition");
    private static readonly AccessTools.FieldRef<Kingmaker.View.CameraZoom, float> ScrollPos = AccessTools.FieldRefAccess<Kingmaker.View.CameraZoom, float>("m_ScrollPosition");
    private static readonly AccessTools.FieldRef<Kingmaker.View.CameraZoom, float> SmoothScroll = AccessTools.FieldRefAccess<Kingmaker.View.CameraZoom, float>("m_SmoothScrollPosition");

    // The rotation tween the player's own input last started on the rig (CameraRig.TickRotate: mouse or keys;
    // AddRotation: gamepad; RotateTo when called by the camera-reset key or compass, SetCameraRotateDefault),
    // recorded by transpilers on those methods. A scripted RotateTo kills it (DOKill) before starting its own
    // and isn't recorded, so a live one at the pin's start is the player's.
    public static DG.Tweening.Tween UserTween;
    private static bool _userRotateTo;

    public static void RotateDefaultPrefix() => _userRotateTo = true;
    public static void RotateDefaultFinalizer() => _userRotateTo = false;

    public static DG.Tweening.Tweener RotateToRotate(Transform target, Vector3 endValue, float duration, DG.Tweening.RotateMode mode)
    {
        var t = DG.Tweening.ShortcutExtensions.DOLocalRotate(target, endValue, duration, mode);
        UserTween = _userRotateTo ? t : null; // a scripted rotation replaces (kills) any user tween
        return t;
    }

    public static IEnumerable<CodeInstruction> RotateToTranspiler(IEnumerable<CodeInstruction> ins) => ReplaceRotate(ins, nameof(RotateToRotate));

    public static DG.Tweening.Tweener UserRotate(Transform target, Vector3 endValue, float duration, DG.Tweening.RotateMode mode)
    {
        var t = DG.Tweening.ShortcutExtensions.DOLocalRotate(target, endValue, duration, mode);
        UserTween = t;
        return t;
    }

    public static IEnumerable<CodeInstruction> UserRotateTranspiler(IEnumerable<CodeInstruction> ins) => ReplaceRotate(ins, nameof(UserRotate));

    private static IEnumerable<CodeInstruction> ReplaceRotate(IEnumerable<CodeInstruction> ins, string replacement)
    {
        var target = AccessTools.Method(typeof(DG.Tweening.ShortcutExtensions), nameof(DG.Tweening.ShortcutExtensions.DOLocalRotate));
        var repl = AccessTools.Method(typeof(InputPin), replacement);
        var n = 0;
        foreach (var i in ins)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodInfo mi && mi == target)
            {
                i.opcode = OpCodes.Call;
                i.operand = repl;
                n++;
            }
            yield return i;
        }
        if (n != 1) Health.Failed("input pin", new Exception($"user camera rotation: {n} DOLocalRotate calls replaced, expected 1"));
    }

    private static void ResetCamera(List<string> notes)
    {
        try
        {
            var rig = Game.Instance?.UI?.GetCameraRig();
            if (rig == null) { notes?.Add("camera: no rig"); return; }
            var rotating = (bool)RigRotationByMouse.GetValue(rig);
            var based = ((Vector3?)RigBaseMousePoint.GetValue(rig)).HasValue;
            if (rotating || based) Game.Instance.CursorController?.SetMoveCameraCursor(state: false);
            RigRotationByMouse.SetValue(rig, false);
            RigBaseMousePoint.SetValue(rig, null);
            RigRotateDistance.SetValue(rig, 0f);
            RigRotateDistance2D.SetValue(rig, Vector2.zero);
            RigScrollBy2D.SetValue(rig, Vector2.zero);
            // Queued key, gamepad and photo-mode camera input (consumed only when no scroll or rotation routine
            // runs, so it can wait across the pin's start).
            RigRotationByKeyboard.SetValue(rig, false);
            RigRotateOffset.SetValue(rig, 0f);
            RigScrollOffset.SetValue(rig, Vector2.zero);
            RigScrollOffsetVertical.SetValue(rig, 0f);
            RigBankRotateOffset.SetValue(rig, 0f);
            // A zoom the player started (wheel, keys, gamepad) still easing in: it stops where it is.
            var zoom = rig.CameraZoom;
            var zoomed = false;
            if (zoom != null && ZoomRoutine(zoom) == null)
            {
                zoomed = PlayerScroll(zoom) != SmoothScroll(zoom) || ScrollPos(zoom) != SmoothScroll(zoom);
                PlayerScroll(zoom) = SmoothScroll(zoom);
                ScrollPos(zoom) = SmoothScroll(zoom);
            }
            notes?.Add($"zoom pending={zoomed}");
            // A rotation the player started still turning the rig: stopped where it is, not completed.
            var tween = UserTween;
            UserTween = null;
            var turning = tween != null && DG.Tweening.TweenExtensions.IsActive(tween) && DG.Tweening.TweenExtensions.IsPlaying(tween);
            if (turning) DG.Tweening.TweenExtensions.Kill(tween, false);
            notes?.Add($"camera: rotating={rotating} base_point={based} user_tween={turning} cleared");
        }
        catch (Exception e) { Health.Failed("input pin: camera reset", e); }
    }

    // UI state a press or drag could leave behind: the EventSystem's input module drops its pointer data
    // (pointer exit, no click or drag end) and the selection (DeactivateModule: ClearSelection), and IMGUI
    // releases any captured control. The module then runs on neutral input from a clean state.
    private static void ResetUi(List<string> notes)
    {
        try
        {
            var es = UnityEngine.EventSystems.EventSystem.current;
            var module = es != null ? es.currentInputModule : null;
            var sel = es != null ? es.currentSelectedGameObject : null;
            module?.DeactivateModule();
            notes?.Add($"ui: module {(module != null ? module.GetType().Name : "-")} cleared, selection {(sel != null ? sel.name : "-")} cleared, imgui hot {GUIUtility.hotControl} keyboard {GUIUtility.keyboardControl}");
            GUIUtility.hotControl = 0;
            GUIUtility.keyboardControl = 0;
        }
        catch (Exception e) { Health.Failed("input pin: ui reset", e); }
    }

    // The window starts: Rewired is taken (else the fixture fails), a hover left from before the pin is
    // cleared through the pointer's own Tick (on pinned input it picks nothing), and the pointer's last world
    // position is the main character's position (the save's).
    public static void WindowOpen()
    {
        if (!_active) return;
        if (!_rewiredDone)
        {
            TryRewired(null);
            if (!_rewiredDone) Health.Failed("input pin", new Exception("Rewired not ready when the window opened"));
        }
        try
        {
            foreach (var pc in Pointers())
            {
                if (pc.PointerOn != null || pc.OvertipObject != null) pc.Tick();
                if (pc.PointerOn != null || pc.OvertipObject != null) Health.Failed("input pin", new Exception("the pointer is still over something at the window's start"));
            }
            var mc = Game.Instance?.Player?.MainCharacter.Value;
            if (mc == null) return;
            foreach (var pc in Pointers())
            {
                SetWorldPosition.Invoke(pc, new object[] { mc.Position });
                SimPos(pc) = mc.Position;
            }
        }
        catch (Exception e) { Health.Failed("input pin: window pointer", e); }
    }

    // What the pin looks like at the window's start (trace header).
    public static string Describe()
    {
        try
        {
            var g = Game.Instance;
            var pc = g?.DefaultPointerController;
            var es = UnityEngine.EventSystems.EventSystem.current;
            var sel = es != null ? es.currentSelectedGameObject : null;
            return $"active={_active}; {BeginNote}; controller_mouse={g?.IsControllerMouse}; pointer_on={(pc?.PointerOn != null ? pc.PointerOn.name : "-")}; overtip={(pc?.OvertipObject != null ? pc.OvertipObject.name : "-")}; mode={pc?.Mode}; in_gui={PointerController.InGui}; ui_selected={(sel != null ? sel.name : "-")}; callers {PatchedCallers}/{CallersFound}";
        }
        catch (Exception e) { return "describe failed: " + e.Message; }
    }

    // The world pick: nothing, or the fixture's hovered unit.
    public static bool SelectClickObjectPrefix(PointerController __instance, out GameObject resultGameObject, out Vector3 worldPosition, out IClickEventHandler resultHandler)
    {
        resultGameObject = null;
        worldPosition = Vector3.zero;
        resultHandler = null;
        if (!_active) return true;
        Picks++;
        try
        {
            GameObject overtip = null;
            var u = Hover;
            var view = u?.View;
            if (view != null)
            {
                var go = view.gameObject;
                var point = u.Position;
                foreach (var h in Handlers(__instance) ?? new IClickEventHandler[0])
                {
                    if (h.GetMode() != __instance.Mode) continue;
                    float priority;
                    try
                    {
                        if (h is IClickAndOvertipEventHandler oh)
                        {
                            var r = oh.ProcessObject(go, point);
                            priority = r.Priority;
                            if (r.ShowOvertip) overtip = go;
                        }
                        else priority = h.GetPriority(go, point);
                    }
                    catch (Exception e) { Health.Log("input pin: handler priority", e); priority = 0f; }
                    if (priority > 0f)
                    {
                        resultGameObject = go;
                        worldPosition = point;
                        resultHandler = h;
                        break;
                    }
                }
            }
            SetOvertip.Invoke(__instance, new object[] { overtip });
        }
        catch (Exception e) { Health.Failed("input pin: pick", e); }
        return false;
    }

    // The fixture points at a unit, or at nothing again; the game's pointer picks it up in its own Tick
    // (hover change, highlight, the click handler).
    // Returns null when the pointer is over the unit with a click handler, else why not.
    public static string PointAt(UnitEntityData u)
    {
        if (u == null)
        {
            Hover = null;
            if (!_active) return null;
            var pc0 = Game.Instance?.DefaultPointerController;
            try { pc0?.Tick(); }
            catch (Exception e) { Health.Failed("input pin: pointer release", e); }
            if (pc0 != null && pc0.PointerOn != null)
            {
                Health.Failed("input pin", new Exception("the pointer is still over " + pc0.PointerOn.name + " after its release"));
                if (Sched.Replay) Sched.Invalidate("the fixture's pointer could not be released");
            }
            return null;
        }
        if (!_active) return "the fixture's pointer is used outside an input-pinned window";
        Hover = u;
        var pc = Game.Instance.DefaultPointerController;
        pc.Tick();
        // The pick runs only outside the UI (PointerController.InGui), and takes a unit only with a handler.
        var view = u.View;
        if (view == null || pc.PointerOn != view.gameObject || SimulateHandler(pc) == null)
            return $"the fixture's pointer did not reach {u.UniqueId}: pointer_on={(pc.PointerOn != null ? pc.PointerOn.name : "-")}, handler={SimulateHandler(pc)?.GetType().Name ?? "-"}, in_gui={PointerController.InGui}";
        return null;
    }

    // Whether a player's pointer can be over u in the default pointer mode: the game's pick resolves the
    // unit's collider and takes the first handler with a priority above 0, ClickUnitHandler, whose priority
    // is 0 exactly when the unit isn't visible to the player, is untargetable, is a personal enemy that
    // can't be targeted now, or (for a living target) is dead. Read passively, without calling the handler
    // (its last branch asks the unit's Group, which runs UpdateGroup; a recording's policy only observes).
    public static bool CanPoint(UnitEntityData u)
    {
        var pc = Game.Instance?.DefaultPointerController;
        if (u?.View == null || pc == null || pc.Mode != PointerMode.Default) return false;
        if (!u.IsVisibleForPlayer || u.State.Features.IsUntargetable || u.Descriptor.State.IsDead) return false;
        var pe = u.Get<Kingmaker.UnitLogic.Parts.UnitPartPersonalEnemy>();
        return pe == null || pe.IsCurrentlyTargetable;
    }

    // ---- held notifications: focus, application pause, controller hot-plug ----
    //
    // The window's focus and a gamepad being plugged in are the machine's state, not the fixture's. Every
    // OnApplicationFocus(bool) / OnApplicationPause(bool) of the game's assemblies (Kingmaker.Runner's tells
    // KeyboardAccess, which fires its key-up bindings on focus loss, and AutoPause; the EventSystem's stops UI
    // processing) and the game's own reactions to Rewired's controller connected / disconnected events
    // (GamepadConnectDisconnectVM, which can pause the game) are held while the pin is active and run, in
    // order and with their arguments, after it ends.
    private static readonly List<(object self, MethodBase method, object[] args)> Held = new List<(object, MethodBase, object[])>();

    public static bool HoldPrefix(object __instance, MethodBase __originalMethod, object[] __args)
    {
        if (!_active) return true;
        lock (Held) Held.Add((__instance, __originalMethod, (object[])__args.Clone()));
        FocusHeld++;
        var what = __originalMethod.DeclaringType?.Name + "." + __originalMethod.Name;
        var arg = __args.Length > 0 ? __args[0]?.ToString() : null;
        Sched.Obs("input_held", j => { j.Prop("what", what); j.Prop("arg", arg); });
        return false;
    }

    private static void DeliverHeld()
    {
        List<(object self, MethodBase method, object[] args)> list;
        lock (Held) { list = new List<(object, MethodBase, object[])>(Held); Held.Clear(); }
        foreach (var (self, method, args) in list)
        {
            if (self is UnityEngine.Object uo && uo == null) continue; // destroyed meanwhile
            if (self != null && AccessTools.Property(self.GetType(), "IsDisposed")?.GetValue(self) is true) continue; // a disposed view model
            try { method.Invoke(self, args); }
            catch (Exception e) { Health.Log("input pin: deliver " + method.Name, e); }
        }
    }

    // IMGUI: OnGUI runs once per event, and the game's IMGUI windows (QA, debug and temporary tools) read
    // hardware mouse and key events from Event.current. While the pin is active every event a game OnGUI sees
    // has the neutral pointer position, no delta and no modifiers, and an input event is marked used (layout
    // and repaint events pass). Begin releases IMGUI's captured controls (ResetUi).
    public static long GuiEventsUsed;

    public static void OnGuiPrefix()
    {
        if (!_active) return;
        var e = Event.current;
        if (e == null) return;
        // Every event (layout and repaint included) carries the neutral pointer, no movement, no modifiers.
        e.mousePosition = new Vector2(NeutralPointer.x, NeutralPointer.y);
        e.delta = Vector2.zero;
        e.modifiers = EventModifiers.None;
        switch (e.type)
        {
            case EventType.MouseDown:
            case EventType.MouseUp:
            case EventType.MouseMove:
            case EventType.MouseDrag:
            case EventType.KeyDown:
            case EventType.KeyUp:
            case EventType.ScrollWheel:
            case EventType.DragUpdated:
            case EventType.DragPerform:
            case EventType.DragExited:
            case EventType.ContextClick:
            case EventType.MouseEnterWindow:
            case EventType.MouseLeaveWindow:
            case EventType.ValidateCommand:
            case EventType.ExecuteCommand:
                e.Use();
                GuiEventsUsed++;
                break;
        }
    }

    // ---- install ----

    private static bool WantAssembly(Assembly a, string managed) =>
        WallClock.WantAssembly(a, managed) && !a.GetName().Name.StartsWith("Rewired", StringComparison.Ordinal); // Rewired: its controllers are disabled

    private static bool IsOpen(MethodBase m) => m.IsGenericMethodDefinition || (m.DeclaringType?.ContainsGenericParameters ?? false);

    // Closed instantiations, in the wanted assemblies, of open generic methods: methods of generic classes
    // closed by a derived class or named in IL with type arguments, generic methods called with type
    // arguments. Instantiations over the caller's own type parameters are skipped (they close elsewhere).
    private static Dictionary<MethodBase, List<MethodBase>> Instantiations(List<MethodBase> open, Func<Assembly, string, bool> want)
    {
        var result = open.ToDictionary(m => m, m => new List<MethodBase>());
        var openTypes = new HashSet<Type>(open.Where(m => m.DeclaringType != null && m.DeclaringType.ContainsGenericParameters).Select(m => m.DeclaringType));
        var openMethods = new HashSet<MethodBase>(open.Where(m => m.IsGenericMethodDefinition));
        var closedTypes = new HashSet<Type>();
        var closedMethods = new HashSet<MethodBase>();
        void SeeType(Type t)
        {
            for (var bt = t; bt != null; bt = bt.BaseType)
                if (bt.IsGenericType && !bt.ContainsGenericParameters && openTypes.Contains(bt.GetGenericTypeDefinition())) closedTypes.Add(bt);
        }
        var managed = System.IO.Path.GetDirectoryName(typeof(Game).Assembly.Location);
        Rng.BuildOpcodeTable();
        foreach (var a in AppDomain.CurrentDomain.GetAssemblies().Where(x => want(x, managed)))
        {
            Type[] types;
            try { types = a.GetTypes(); } catch (ReflectionTypeLoadException e) { types = e.Types.Where(t => t != null).ToArray(); }
            catch (Exception) { continue; }
            var module = a.ManifestModule;
            var seen = new HashSet<int>();
            foreach (var t in types)
            {
                SeeType(t);
                const BindingFlags all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
                IEnumerable<MethodBase> methods;
                try { methods = t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)); } catch (Exception) { continue; }
                foreach (var m in methods)
                {
                    byte[] il;
                    try { il = m.GetMethodBody()?.GetILAsByteArray(); } catch (Exception) { continue; }
                    if (il == null) continue;
                    var pos = 0;
                    while (pos < il.Length)
                    {
                        OpCode op;
                        var b = il[pos++];
                        if (b == 0xfe) { if (pos >= il.Length) break; op = Rng.Two[il[pos++]]; }
                        else op = Rng.One[b];
                        if (op.Size == 0) break;
                        var operand = 0;
                        switch (op.OperandType)
                        {
                            case OperandType.InlineNone: break;
                            case OperandType.ShortInlineBrTarget:
                            case OperandType.ShortInlineI:
                            case OperandType.ShortInlineVar: pos += 1; break;
                            case OperandType.InlineVar: pos += 2; break;
                            case OperandType.InlineI8:
                            case OperandType.InlineR: pos += 8; break;
                            case OperandType.InlineSwitch: pos += 4 + 4 * BitConverter.ToInt32(il, pos); break;
                            default: operand = BitConverter.ToInt32(il, pos); pos += 4; break;
                        }
                        if (op.OperandType != OperandType.InlineMethod) continue;
                        var kind = (uint)operand >> 24;
                        if (kind != 0x0A && kind != 0x2B) continue; // MemberRef (a method of a constructed type) or MethodSpec
                        if (!seen.Add(operand)) continue;
                        MethodBase callee;
                        try { callee = module.ResolveMethod(operand); }
                        catch (Exception) { continue; } // instantiated over the caller's type parameters
                        if (callee == null || callee.ContainsGenericParameters) continue;
                        if (callee.DeclaringType != null) SeeType(callee.DeclaringType);
                        if (callee.IsGenericMethod && openMethods.Contains(((MethodInfo)callee).GetGenericMethodDefinition())) closedMethods.Add(callee);
                    }
                }
            }
        }
        foreach (var m in open)
        {
            if (m.IsGenericMethodDefinition)
            {
                if (m.DeclaringType != null && m.DeclaringType.ContainsGenericParameters) continue; // generic method of a generic class: none here
                result[m].AddRange(closedMethods.Where(c => ((MethodInfo)c).GetGenericMethodDefinition() == (MethodInfo)m));
            }
            else
            {
                foreach (var ct in closedTypes.Where(ct => ct.GetGenericTypeDefinition() == m.DeclaringType))
                {
                    try { result[m].Add(MethodBase.GetMethodFromHandle(m.MethodHandle, ct.TypeHandle)); }
                    catch (Exception e) { Health.Failed("input pin: instantiate " + m.Name + " in " + ct, e); }
                }
            }
        }
        return result;
    }

    public static void Install(Harmony h)
    {
        var sw = Stopwatch.StartNew();
        foreach (var (type, member, args, repl) in Members)
        {
            var m = type.GetMethod(member, BindingFlags.Public | BindingFlags.Static, null, args, null);
            var r = AccessTools.Method(typeof(InputPin), repl);
            if (m == null || r == null) { Health.Failed("input pin: member", new Exception($"{type.Name}.{member} or {repl} missing")); continue; }
            Map[(m.Module, m.MetadataToken)] = r;
        }
        var found = WallClock.FindCallers(IsReplaced, WantAssembly, ldftn: true);
        CallersFound = found.Count;
        // An open generic (a generic method, or a method of a generic class) can't be patched itself; its
        // closed instantiations in the game's code are (a class closing the generic base, a call naming
        // the type arguments). One with none in the game's IL is never run.
        var callers = found.Where(m => !IsOpen(m)).ToList();
        var open = found.Where(IsOpen).ToList();
        var unused = new List<string>();
        if (open.Count > 0)
        {
            var closed = Instantiations(open, WantAssembly);
            foreach (var m in open)
            {
                var list = closed.TryGetValue(m, out var l) ? l : new List<MethodBase>();
                if (list.Count == 0) unused.Add(m.DeclaringType?.FullName + "." + m.Name);
                callers.AddRange(list);
            }
        }
        var tr = new HarmonyMethod(typeof(InputPin), nameof(Transpiler));
        var failed = new List<string>();
        var byAssembly = new Dictionary<string, int>();
        foreach (var m in callers)
        {
            var name = m.DeclaringType?.FullName + "." + m.Name;
            try
            {
                h.Patch(m, transpiler: tr);
                PatchedCallers++;
                var an = m.DeclaringType?.Assembly.GetName().Name ?? "?";
                byAssembly.TryGetValue(an, out var n);
                byAssembly[an] = n + 1;
            }
            catch (Exception e)
            {
                FailedCallers++;
                failed.Add(name);
                Health.Failed("input pin: transpile " + name, e);
            }
        }
        try
        {
            h.Patch(AccessTools.Method(typeof(PointerController), nameof(PointerController.SelectClickObject)),
                prefix: new HarmonyMethod(typeof(InputPin), nameof(SelectClickObjectPrefix)));
        }
        catch (Exception e) { Health.Failed("input pin: patch pointer pick", e); }
        // Held notifications and IMGUI handlers, by name in the same assemblies.
        var hold = new HarmonyMethod(typeof(InputPin), nameof(HoldPrefix));
        var gui = new HarmonyMethod(typeof(InputPin), nameof(OnGuiPrefix));
        int held = 0, guis = 0;
        var managed = System.IO.Path.GetDirectoryName(typeof(Game).Assembly.Location);
        foreach (var a in AppDomain.CurrentDomain.GetAssemblies().Where(x => WallClock.WantAssembly(x, managed)))
        {
            Type[] types;
            try { types = a.GetTypes(); } catch (ReflectionTypeLoadException e) { types = e.Types.Where(t => t != null).ToArray(); }
            catch (Exception) { continue; }
            foreach (var t in types)
            {
                if (t.ContainsGenericParameters) continue;
                MethodInfo[] ms;
                try { ms = t.GetMethods(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly); } catch (Exception) { continue; }
                foreach (var m in ms)
                {
                    var ps = m.GetParameters();
                    var isFocus = (m.Name == "OnApplicationFocus" || m.Name == "OnApplicationPause") && ps.Length == 1 && ps[0].ParameterType == typeof(bool) && m.ReturnType == typeof(void);
                    var isGui = m.Name == "OnGUI" && ps.Length == 0 && typeof(MonoBehaviour).IsAssignableFrom(t) && m.GetMethodBody() != null;
                    if (!isFocus && !isGui) continue;
                    try
                    {
                        if (isFocus) { h.Patch(m, prefix: hold); held++; }
                        else { h.Patch(m, prefix: gui); guis++; }
                    }
                    catch (Exception e) { Health.Failed("input pin: patch " + t.FullName + "." + m.Name, e); }
                }
            }
        }
        foreach (var name in new[] { nameof(Kingmaker.View.CameraRig.TickRotate), nameof(Kingmaker.View.CameraRig.AddRotation) })
        {
            try { h.Patch(AccessTools.Method(typeof(Kingmaker.View.CameraRig), name), transpiler: new HarmonyMethod(typeof(InputPin), nameof(UserRotateTranspiler))); }
            catch (Exception e) { Health.Failed("input pin: patch CameraRig." + name, e); }
        }
        try
        {
            h.Patch(AccessTools.Method(typeof(Kingmaker.View.CameraRig), nameof(Kingmaker.View.CameraRig.RotateTo)), transpiler: new HarmonyMethod(typeof(InputPin), nameof(RotateToTranspiler)));
            h.Patch(AccessTools.Method(typeof(Kingmaker.View.CameraRig), nameof(Kingmaker.View.CameraRig.SetCameraRotateDefault)),
                prefix: new HarmonyMethod(typeof(InputPin), nameof(RotateDefaultPrefix)), finalizer: new HarmonyMethod(typeof(InputPin), nameof(RotateDefaultFinalizer)));
        }
        catch (Exception e) { Health.Failed("input pin: patch CameraRig.RotateTo", e); }
        foreach (var name in new[] { "OnGamepadConnected", "OnGamepadDisconnected" })
        {
            try { h.Patch(AccessTools.Method(typeof(Kingmaker.UI.MVVM._VM.ChoseControllerMode.GamepadConnectDisconnectVM), name), prefix: hold); held++; }
            catch (Exception e) { Health.Failed("input pin: patch GamepadConnectDisconnectVM." + name, e); }
        }
        ScanNote = $"input pin: {CallersFound} methods read UnityEngine.Input ({open.Count} open generics: {callers.Count - (CallersFound - open.Count)} closed instantiations, {unused.Count} never instantiated: {string.Join(", ", unused)}), {PatchedCallers} transpiled ({string.Join(", ", byAssembly.OrderBy(kv => kv.Key, StringComparer.Ordinal).Select(kv => kv.Key + " " + kv.Value))}), {FailedCallers} failed ({string.Join(", ", failed.Take(8))}); {Map.Count} Input/Event members replaced; {held} focus/pause/hot-plug handlers held, {guis} OnGUI handlers; {sw.Elapsed.TotalSeconds:0.00} s";
        Fidelity.Notes.Add(ScanNote);
    }
}
