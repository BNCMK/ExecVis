// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: Workload.cs
//  script_path: execviz-dotnet/Workload.cs
//  module_name: Workload
//  version: 0.53.1
//  description: A .NET service, traced: requests fan in across tasks, a worker claims stamped work off a channel, one request fails, and a lock never releases.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: Workload
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// A .NET service, traced: requests fan in across tasks, a worker claims stamped
// work off a channel, one request fails, and a lock never releases.
//
// The point of interest is that every child below is created inside an `await`
// chain and resumed on whichever pool thread the runtime chose. If the carrier
// were a ThreadLocal, these would attach to whatever else that thread had been
// doing.
using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace Execviz
{
    public static class Workload
    {
        public static async Task<int> Main()
        {
            return await RealMain();
        }

        public static async Task<int> RealMain()
        {
            Capture.Install(Environment.GetEnvironmentVariable("EXECVIZ_COLLECTOR"),
                            "dotnet-1", "api");

            var root = Capture.SpanStart("service", "call");
            var stuck = Capture.SpanStart("reconcile_lock", "wait", domain: "billing");
            Capture.Lifecycle(stuck, "suspended");   // never released: an unfinished span

            var jobs = new List<Dictionary<string, object?>>();

            for (var uid = 0; uid < 3; uid++)
            {
                var id = uid;
                var prev = Capture.CurrentSpan();
                var reqId = Capture.SpanStart($"GET /profile/{id}", "call", parent: root);
                try
                {
                    await RunUnder(reqId, async () =>
                    {
                        await Capture.Gather("profile_fanin",
                            async () => await Capture.WithSpan("fetch_user", "call", async () =>
                            {
                                Capture.Log("info", "loading user");
                                await Capture.WithSpan("db_user", "io", async () =>
                                    await Task.Delay(40));
                            }, domain: "users"),
                            async () => await Capture.WithSpan("fetch_orders", "call", async () =>
                            {
                                await Capture.WithSpan("db_orders", "io", async () =>
                                {
                                    await Task.Delay(60);
                                    if (id == 2)
                                    {
                                        Capture.Log("error", "order store unavailable");
                                        throw new InvalidOperationException("order store unavailable",
                                            new TimeoutException("no reply within 60ms"));
                                    }
                                });
                            }, domain: "orders"));

                        await Capture.WithSpan("render", "call", async () => await Task.Delay(20),
                            domain: "render");

                        var q = Capture.SpanStart("enqueue_job", "queue");
                        var msg = Capture.Stamp($"invoice-{id}");
                        msg["span"] = q;
                        jobs.Add(msg);
                    });
                    Capture.SpanEnd(reqId, "ok");
                }
                catch (Exception ex) { Capture.SpanEnd(reqId, "error", error: ex); }
            }

            // a worker on a different task: it claims what it is handed
            await Task.Run(async () =>
            {
                foreach (var msg in jobs)
                {
                    var (item, qs) = Capture.Claim(msg);
                    await Capture.WithSpan($"process_{item}", "call", async () =>
                    {
                        Capture.Log("info", $"processing {item}");
                        await Task.Delay(30);
                    }, domain: "worker");
                    Capture.Release(qs);
                }
            });

            Capture.SpanEnd(root, "ok");
            Capture.Flush();
            Console.WriteLine("dotnet workload complete");
            return 0;
        }

        /// Runs work with a given span current. Exists because AsyncLocal does
        /// not flow out of an async method, so a span opened for a caller must
        /// be made current around the work explicitly.
        private static async Task RunUnder(string spanId, Func<Task> work)
        {
            await Capture.WithSpanCurrent(spanId, work).ConfigureAwait(false);
        }
    }
}
