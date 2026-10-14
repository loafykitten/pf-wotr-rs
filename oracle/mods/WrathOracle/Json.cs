using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using Newtonsoft.Json;
using UnityEngine;

namespace WrathOracle;

// Streaming JSON output (the game's Newtonsoft). Floats are written round-trip ("R"), so
// another engine reads back exactly the values the game used. Vectors are [x,y,z],
// quaternions [x,y,z,w], matrices row-major [[m00,m01,m02,m03],...] (matrix[row,col]).
internal sealed class Json : IDisposable
{
    public readonly JsonTextWriter W;
    private readonly TextWriter _stream;
    public readonly string Path; // null for an in-memory writer

    public Json(string path, bool indent = false)
        : this(OpenFile(path), indent)
    {
        Path = path;
    }

    public Json(TextWriter writer, bool indent = false)
    {
        _stream = writer;
        W = new JsonTextWriter(_stream)
        {
            Formatting = indent ? Formatting.Indented : Formatting.None,
            FloatFormatHandling = FloatFormatHandling.String,
        };
    }

    private static TextWriter OpenFile(string path)
    {
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(path));
        return new StreamWriter(path, false, new UTF8Encoding(false), 1 << 16);
    }

    public void Dispose()
    {
        W.Flush();
        W.Close();
        _stream.Dispose();
        if (Path != null) Health.FileWritten(Path);
    }

    // Writes one record built by `write` into this writer's current array/property.
    // The record is built in memory first, so a failure leaves no half-written JSON:
    // it's counted in Health and replaced by {"$failed": "..."}.
    public void Record(string what, Action<Json> write)
    {
        string text;
        try
        {
            var sw = new StringWriter(System.Globalization.CultureInfo.InvariantCulture);
            using (var sub = new Json(sw)) write(sub);
            text = sw.ToString();
        }
        catch (Exception e)
        {
            Health.Failed(what, e);
            W.WriteStartObject();
            W.WritePropertyName("$failed");
            W.WriteValue(what + ": " + e.GetType().Name + ": " + e.Message);
            W.WriteEndObject();
            return;
        }
        W.WriteRawValue(text);
        Health.Written();
    }

    public Json Obj() { W.WriteStartObject(); return this; }
    public Json End() { W.WriteEndObject(); return this; }
    public Json Arr() { W.WriteStartArray(); return this; }
    public Json EndArr() { W.WriteEndArray(); return this; }
    public Json Key(string k) { W.WritePropertyName(k); return this; }
    public Json Obj(string k) { W.WritePropertyName(k); W.WriteStartObject(); return this; }
    public Json Arr(string k) { W.WritePropertyName(k); W.WriteStartArray(); return this; }

    public Json Prop(string k, string v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, bool v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, int v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, long v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, uint v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, float v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, double v) { W.WritePropertyName(k); W.WriteValue(v); return this; }
    public Json Prop(string k, bool? v) { W.WritePropertyName(k); if (v.HasValue) W.WriteValue(v.Value); else W.WriteNull(); return this; }
    public Json Prop(string k, Vector3 v) { W.WritePropertyName(k); Vec(v); return this; }
    public Json Prop(string k, Vector2 v) { W.WritePropertyName(k); W.WriteStartArray(); W.WriteValue(v.x); W.WriteValue(v.y); W.WriteEndArray(); return this; }
    public Json Prop(string k, Quaternion q) { W.WritePropertyName(k); Quat(q); return this; }
    public Json Prop(string k, Matrix4x4 m) { W.WritePropertyName(k); Mat(m); return this; }
    public Json Prop(string k, Bounds b) { W.WritePropertyName(k); W.WriteStartObject(); Prop("center", b.center); Prop("size", b.size); W.WriteEndObject(); return this; }
    public Json Prop(string k, Rect r) { W.WritePropertyName(k); W.WriteStartArray(); W.WriteValue(r.x); W.WriteValue(r.y); W.WriteValue(r.width); W.WriteValue(r.height); W.WriteEndArray(); return this; }
    public Json Null(string k) { W.WritePropertyName(k); W.WriteNull(); return this; }

    public void Vec(Vector3 v) { W.WriteStartArray(); W.WriteValue(v.x); W.WriteValue(v.y); W.WriteValue(v.z); W.WriteEndArray(); }
    public void Quat(Quaternion q) { W.WriteStartArray(); W.WriteValue(q.x); W.WriteValue(q.y); W.WriteValue(q.z); W.WriteValue(q.w); W.WriteEndArray(); }

    public void Mat(Matrix4x4 m)
    {
        W.WriteStartArray();
        for (var r = 0; r < 4; r++)
        {
            W.WriteStartArray();
            for (var c = 0; c < 4; c++) W.WriteValue(m[r, c]);
            W.WriteEndArray();
        }
        W.WriteEndArray();
    }

    // Local and world transform of a Transform.
    public Json Transform(string k, Transform t)
    {
        Obj(k);
        Prop("local_position", t.localPosition);
        Prop("local_rotation", t.localRotation);
        Prop("local_scale", t.localScale);
        Prop("position", t.position);
        Prop("rotation", t.rotation);
        Prop("lossy_scale", t.lossyScale);
        Prop("local_to_world", t.localToWorldMatrix);
        End();
        return this;
    }

    public static string Sha256File(string path)
    {
        try
        {
            using (var sha = SHA256.Create())
            using (var f = File.OpenRead(path))
                return Hex(sha.ComputeHash(f));
        }
        catch (Exception e)
        {
            return "error: " + e.Message;
        }
    }

    public static string Sha256(byte[] data)
    {
        using (var sha = SHA256.Create()) return Hex(sha.ComputeHash(data));
    }

    private static string Hex(byte[] b)
    {
        var sb = new StringBuilder(b.Length * 2);
        foreach (var x in b) sb.Append(x.ToString("x2"));
        return sb.ToString();
    }
}

// Recorder health (Wrath-Rust PLAN: a broken recorder must not pass silently). Written:
// records emitted; failed: records whose capture threw (logged, then skipped); dropped:
// records lost for capacity (e.g. frames past a preallocated buffer). Any thread.
internal static class Health
{
    private static long _written, _failed, _dropped, _files;
    private static readonly List<string> Errors = new List<string>();
    private static readonly List<string> Files = new List<string>();

    public static long WrittenCount => Interlocked.Read(ref _written);
    public static long FailedCount => Interlocked.Read(ref _failed);
    public static long DroppedCount => Interlocked.Read(ref _dropped);

    public static void Written(long n = 1) => Interlocked.Add(ref _written, n);
    public static void Dropped(long n = 1) => Interlocked.Add(ref _dropped, n);

    public static void Failed(string what, Exception e)
    {
        Interlocked.Increment(ref _failed);
        lock (Errors)
        {
            if (Errors.Count < 200) Errors.Add($"{what}: {e.GetType().Name}: {e.Message}");
        }
    }

    // Logged, not counted as a failed record: snapshot value reads have their own counter
    // (Snap.ValueFailures), so the two never mix.
    public static void Log(string what, Exception e)
    {
        lock (Errors)
        {
            if (Errors.Count < 200) Errors.Add($"{what}: {e.GetType().Name}: {e.Message}");
        }
    }

    public static void FileWritten(string path)
    {
        Interlocked.Increment(ref _files);
        lock (Files) Files.Add(path);
    }

    public static void Write(Json j)
    {
        j.Obj("health");
        j.Prop("records_written", WrittenCount);
        j.Prop("records_failed", FailedCount);
        j.Prop("records_dropped", DroppedCount);
        j.Prop("files_written", Interlocked.Read(ref _files));
        j.Arr("errors");
        lock (Errors) foreach (var e in Errors) j.W.WriteValue(e);
        j.EndArr();
        j.End();
    }
}
