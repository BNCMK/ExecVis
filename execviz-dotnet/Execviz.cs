// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: Execviz.cs
//  script_path: execviz-dotnet/Execviz.cs
//  module_name: Execviz
//  version: 0.53.1
//  description: execviz capture adapter for .NET.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: Execviz, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// execviz capture adapter for .NET.
//
// The carrier is AsyncLocal<T>. Like Python's contextvars and Node's
// AsyncLocalStorage it flows with the logical call, not with the thread, which
// makes it correct across `await`: a continuation resumed on a pooled
// thread still sees the span its caller started.
//
// The failure mode worth naming is the opposite of the JVM's. A ThreadLocal on
// a pooled thread *leaks* a stale span into unrelated work; AsyncLocal does not,
// because the value is captured into the ExecutionContext at the await point
// rather than living on the thread. What AsyncLocal does not do is flow *out*
// of a context; a value set inside an async method is invisible to its caller,
// because every span is opened and closed within one scope here.
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Text.Json;
using System.IO;
using System.Threading;
using System.Threading.Tasks;

namespace Execviz
{
    public sealed class Span
    {
        public string SpanId { get; set; } = "";
        public string TraceId { get; set; } = "";
        public string? ParentSpanId { get; set; }
        public List<string> Links { get; } = new();
        public string Name { get; set; } = "";
        public string Kind { get; set; } = "call";
        public double Start { get; set; }
        public double? End { get; set; }
        public string Status { get; set; } = "running";
        public string HostId { get; set; } = "dotnet";
        public string? Domain { get; set; }
        public List<object> Lifecycle { get; } = new();
        public List<object> Events { get; } = new();
        public Dictionary<string, object?> Attributes { get; } = new();
        public object? Error { get; set; }
    }

    public static class Capture
    {
        // AsyncLocal flows with the logical call. This is the whole adapter's
        // correctness resting on one type.
        private static readonly AsyncLocal<string?> Current = new();
        private static readonly AsyncLocal<string?> Trace = new();

        private static readonly ConcurrentDictionary<string, Span> Pending = new();
        private static readonly ConcurrentDictionary<string, string> Sent = new();
        private static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(8) };
        private static string _collector = "http://127.0.0.1:8900";
        private static string _host = "dotnet-1";
        private static string _domain = "app";
        private static Timer? _timer;

        public static void Install(string? collector = null, string host = "dotnet-1", string domain = "app")
        {
            _collector = collector
                ?? Environment.GetEnvironmentVariable("EXECVIZ_COLLECTOR")
                ?? "http://127.0.0.1:8900";
            _host = host;
            _domain = domain;
            Trace.Value = Sid();
            // periodic delivery, and an explicit Flush for shutdown: a process
            // that exits without flushing loses exactly the record of why
            _timer = new Timer(_ => Deliver(), null, 700, 700);
        }

        public static void SetDomain(string d) => _domain = d;
        public static string? CurrentSpan() => Current.Value;

        public static string SpanStart(string name, string kind = "call",
            string? parent = null, IEnumerable<string>? links = null,
            string? domain = null, IDictionary<string, object?>? attributes = null)
        {
            var id = Sid();
            var s = new Span
            {
                SpanId = id,
                TraceId = Trace.Value ??= Sid(),
                ParentSpanId = parent ?? Current.Value,
                Name = name,
                Kind = kind,
                Start = Now(),
                HostId = _host,
                Domain = domain ?? _domain,
            };
            if (links != null) s.Links.AddRange(links);
            s.Attributes["thread"] = Environment.CurrentManagedThreadId;
            if (attributes != null) foreach (var kv in attributes) s.Attributes[kv.Key] = kv.Value;
            Pending[id] = s;
            return id;
        }

        public static void SpanEnd(string id, string status = "ok",
            IDictionary<string, object?>? attributes = null, Exception? error = null)
        {
            if (!Pending.TryGetValue(id, out var s)) return;
            s.End = Now();
            s.Status = status;
            if (attributes != null) foreach (var kv in attributes) s.Attributes[kv.Key] = kv.Value;
            if (error != null) s.Error = DescribeError(error);
        }

        public static void Lifecycle(string id, string type, object? context = null)
        {
            if (!Pending.TryGetValue(id, out var s)) return;
            var e = new Dictionary<string, object?> { ["t"] = Now(), ["type"] = type };
            if (context != null) e["context"] = context;
            s.Lifecycle.Add(e);
        }

        /// A log line belongs to the span that was running when it was written.
        public static void Log(string level, string message)
        {
            var id = Current.Value;
            if (id == null || !Pending.TryGetValue(id, out var s)) return;
            s.Events.Add(new Dictionary<string, object?>
                { ["t"] = Now(), ["level"] = level, ["msg"] = message });
        }

        /// Type, message, frames and the cause chain beneath.
        /// The chain matters most: the exception on top is usually the least
        /// informative one in the stack.
        private static object DescribeError(Exception e)
        {
            var frames = new StackTrace(e, true).GetFrames()?
                .Take(12)
                .Select(f => new Dictionary<string, object?>
                {
                    ["file"] = f.GetFileName(),
                    ["line"] = f.GetFileLineNumber(),
                    ["func"] = f.GetMethod()?.Name,
                })
                .ToList() ?? new List<Dictionary<string, object?>>();
            var chain = new List<object>();
            var cur = e.InnerException;
            var guard = 0;
            while (cur != null && guard++ < 8)
            {
                chain.Add(new Dictionary<string, object?>
                    { ["type"] = cur.GetType().Name, ["message"] = cur.Message });
                cur = cur.InnerException;
            }
            var err = new Dictionary<string, object?>
            {
                ["type"] = e.GetType().Name,
                ["message"] = e.Message,
                ["frames"] = frames,
            };
            if (chain.Count > 0) err["caused_by"] = chain;
            return err;
        }

        /// Runs the work inside a span, with that span active for what it reaches.
        /// The span is opened and closed in one scope because AsyncLocal does not
        /// flow out of an async method back to its caller.
        public static async Task<T> WithSpan<T>(string name, string kind, Func<Task<T>> work,
            string? domain = null)
        {
            var id = SpanStart(name, kind, domain: domain);
            var prev = Current.Value;
            Current.Value = id;
            try
            {
                var r = await work().ConfigureAwait(false);
                SpanEnd(id, "ok");
                return r;
            }
            catch (Exception ex)
            {
                SpanEnd(id, "error", error: ex);
                throw;
            }
            finally { Current.Value = prev; }
        }

        /// Makes an already-open span current for the duration of some work.
        /// AsyncLocal does not flow out of an async method back to its caller,
        /// so a span opened on behalf of a caller has to be made current around
        /// the work rather than merely assigned.
        public static async Task WithSpanCurrent(string spanId, Func<Task> work)
        {
            var prev = Current.Value;
            Current.Value = spanId;
            try { await work().ConfigureAwait(false); }
            finally { Current.Value = prev; }
        }

        public static Task WithSpan(string name, string kind, Func<Task> work, string? domain = null)
            => WithSpan<object?>(name, kind, async () => { await work().ConfigureAwait(false); return null; }, domain);

        /// A fan-in: the join keeps the enclosing scope as its parent and names
        /// every child in links. Parenting it to a child would place
        /// it outside its own parent in time.
        public static async Task Gather(string name, params Func<Task>[] work)
        {
            var parent = Current.Value;
            var ids = new List<string>();
            var tasks = new List<Task>();
            for (var i = 0; i < work.Length; i++)
            {
                var id = SpanStart($"{name}[{i}]", "call", parent: parent);
                ids.Add(id);
                var w = work[i];
                tasks.Add(Task.Run(async () =>
                {
                    Current.Value = id;
                    try { await w().ConfigureAwait(false); SpanEnd(id, "ok"); }
                    catch (Exception ex) { SpanEnd(id, "error", error: ex); }
                }));
            }
            await Task.WhenAll(tasks).ConfigureAwait(false);
            var join = SpanStart($"{name}_join", "call", parent: parent, links: ids);
            SpanEnd(join, "ok");
        }

        /// Context stamped onto a value crossing a boundary, read back on the
        /// far side. A queue is where a carrier cannot follow, so it is explicit.
        public static Dictionary<string, object?> Stamp(object? item) => new()
        {
            ["item"] = item, ["trace_id"] = Trace.Value, ["span"] = Current.Value,
        };

        public static (object? Item, string? QueueSpan) Claim(Dictionary<string, object?> msg)
        {
            var qs = msg.TryGetValue("span", out var v) ? v as string : null;
            if (msg.TryGetValue("trace_id", out var t) && t is string tid) Trace.Value = tid;
            if (qs != null)
            {
                Lifecycle(qs, "claimed");
                Current.Value = qs;
            }
            return (msg.TryGetValue("item", out var it) ? it : null, qs);
        }

        public static void Release(string? queueSpan)
        {
            if (queueSpan == null) return;
            Lifecycle(queueSpan, "released");
            SpanEnd(queueSpan, "ok");
            Current.Value = null;
        }

        /// A span is re-sent once its second phase lands; a failed delivery is
        /// retried rather than dropped.
        public static void Deliver()
        {
            var batch = Pending.Values
                .Where(s => !Sent.TryGetValue(s.SpanId, out var st) || st != PhaseOf(s))
                .ToList();
            if (batch.Count == 0) return;
            try
            {
                var body = JsonSerializer.Serialize(new Dictionary<string, object?>
                {
                    ["host_id"] = _host,
                    ["spans"] = batch.Select(ToWire).ToList(),
                });
                var res = Http.PostAsync(_collector + "/api/ingest",
                    new StringContent(body, Encoding.UTF8, "application/json")).GetAwaiter().GetResult();
                if (!res.IsSuccessStatusCode) return;
                ReportRefusals(res.Content.ReadAsStringAsync().GetAwaiter().GetResult());
                foreach (var s in batch)
                {
                    Sent[s.SpanId] = PhaseOf(s);
                    if (s.End != null) Pending.TryRemove(s.SpanId, out _);
                }
            }
            catch { /* never fail the program being observed because recording failed */ }
        }

        /// <summary>
        /// Reads what the collector said about the batch.
        ///
        /// It names every span it refused and why. That explanation reached
        /// nobody while the reply was discarded and any 200 treated as complete
        /// success, so an adapter emitting malformed spans went on emitting them
        /// with nothing to show its author.
        ///
        /// Reported once per distinct reason: a bug in an adapter repeats every
        /// second, and a message that repeats with it is one nobody reads.
        /// </summary>
        private static readonly ConcurrentDictionary<string, bool> ReportedRefusals = new();
        private static long _refused;

        public static long RefusedByCollector() => Interlocked.Read(ref _refused);

        private static void ReportRefusals(string body)
        {
            try
            {
                using var doc = JsonDocument.Parse(body);
                if (!doc.RootElement.TryGetProperty("rejected", out var r) || r.GetInt32() <= 0) return;
                Interlocked.Add(ref _refused, r.GetInt32());
                if (!doc.RootElement.TryGetProperty("reasons", out var reasons)) return;
                foreach (var el in reasons.EnumerateArray())
                {
                    var reason = el.GetString();
                    if (string.IsNullOrEmpty(reason)) continue;
                    // the span id changes every time, so key on the explanation itself
                    var colon = reason.IndexOf(':');
                    var key = colon >= 0 ? reason[(colon + 1)..].Trim() : reason;
                    if (!ReportedRefusals.TryAdd(key, true)) continue;
                    Console.Error.WriteLine($"execviz: the collector refused a span; {reason}");
                    Console.Error.WriteLine("  (further spans refused for this reason will not be reported again)");
                }
            }
            catch
            {
                // an unreadable reply must never break delivery
            }
        }

        public static void Flush()
        {
            _timer?.Change(Timeout.Infinite, Timeout.Infinite);
            Deliver();
        }

        private static string PhaseOf(Span s) => (s.End != null ? "1" : "0") + "|" + s.Status;

        private static Dictionary<string, object?> ToWire(Span s)
        {
            var d = new Dictionary<string, object?>
            {
                ["span_id"] = s.SpanId,
                ["trace_id"] = s.TraceId,
                ["parent_span_id"] = s.ParentSpanId,
                ["links"] = s.Links,
                ["name"] = s.Name,
                ["kind"] = s.Kind,
                ["start"] = s.Start,
                ["end"] = s.End,
                ["status"] = s.Status,
                ["lifecycle"] = s.Lifecycle,
                ["events"] = s.Events,
                ["origin"] = "semantic",
                // which clock stamped this, so skew analysis knows what it compares
                ["clock_source"] = "DateTimeOffset.UtcNow",
                ["host_id"] = s.HostId,
                ["domain"] = s.Domain,
                ["attributes"] = s.Attributes,
            };
            if (s.Error != null) d["error"] = s.Error;
            return d;
        }

        private static double Now() =>
            DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() / 1000.0;

        private static string Sid()
        {
            Span<byte> b = stackalloc byte[6];
            System.Security.Cryptography.RandomNumberGenerator.Fill(b);
            return Convert.ToHexString(b).ToLower(CultureInfo.InvariantCulture);
        }
    }
}
