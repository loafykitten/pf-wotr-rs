using System;
using System.IO;

namespace WrathStats.Core;

// Allocation-free text formatting for the writer thread. TextWriter.Write(int) and
// double.ToString() allocate a string per value; this formats into a reusable buffer.
internal sealed class LineBuilder
{
    private char[] _buf = new char[1024];
    private int _len;

    public void Clear() => _len = 0;

    public void WriteTo(TextWriter writer) => writer.Write(_buf, 0, _len);

    public LineBuilder Append(char c)
    {
        Ensure(1);
        _buf[_len++] = c;
        return this;
    }

    public LineBuilder Append(string s)
    {
        if (s == null) return this;
        Ensure(s.Length);
        s.CopyTo(0, _buf, _len, s.Length);
        _len += s.Length;
        return this;
    }

    public LineBuilder Append(bool b) => Append(b ? '1' : '0');

    public LineBuilder Append(long v)
    {
        if (v < 0)
        {
            if (v == long.MinValue) return Append("-9223372036854775808");
            Append('-');
            v = -v;
        }
        Ensure(20);
        var start = _len;
        do
        {
            _buf[_len++] = (char)('0' + (int)(v % 10));
            v /= 10;
        } while (v != 0);
        Array.Reverse(_buf, start, _len - start);
        return this;
    }

    // Fixed-point, e.g. Append(16.6667, 2) -> "16.67". NaN/infinity -> empty field.
    public LineBuilder Append(double v, int decimals)
    {
        if (double.IsNaN(v) || double.IsInfinity(v)) return this;
        long scale = 1;
        for (var i = 0; i < decimals; i++) scale *= 10;
        var scaled = (long)Math.Round(Math.Abs(v) * scale, MidpointRounding.AwayFromZero);
        if (v < 0 && scaled != 0) Append('-');
        Append(scaled / scale);
        if (decimals == 0) return this;
        Append('.');
        var frac = scaled % scale;
        for (var div = scale / 10; div > 0; div /= 10)
            Append((char)('0' + (int)(frac / div % 10)));
        return this;
    }

    // CSV field: quoted only when needed.
    public LineBuilder AppendCsv(string s)
    {
        if (s == null) return this;
        var needsQuotes = s.IndexOfAny(CsvSpecial) >= 0;
        if (!needsQuotes) return Append(s);
        Append('"');
        foreach (var c in s)
        {
            if (c == '"') Append('"');
            Append(c);
        }
        return Append('"');
    }

    // JSON string literal including quotes; null -> null.
    public LineBuilder AppendJson(string s)
    {
        if (s == null) return Append("null");
        Append('"');
        foreach (var c in s)
        {
            switch (c)
            {
                case '"': Append("\\\""); break;
                case '\\': Append("\\\\"); break;
                case '\n': Append("\\n"); break;
                case '\r': Append("\\r"); break;
                case '\t': Append("\\t"); break;
                default:
                    if (c < 0x20)
                    {
                        Append("\\u00");
                        Append(Hex[c >> 4]).Append(Hex[c & 0xF]);
                    }
                    else
                    {
                        Append(c);
                    }
                    break;
            }
        }
        return Append('"');
    }

    private static readonly char[] CsvSpecial = { ',', '"', '\n', '\r' };
    private const string Hex = "0123456789abcdef";

    private void Ensure(int extra)
    {
        if (_len + extra <= _buf.Length) return;
        var bigger = new char[Math.Max(_buf.Length * 2, _len + extra)];
        Array.Copy(_buf, bigger, _len);
        _buf = bigger;
    }
}
