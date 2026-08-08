// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: main.go
//  script_path: execviz-go/cmd/workload/main.go
//  module_name: main
//  version: 0.53.1
//  description: A real Go service, traced. Goroutines interleave, a worker claims stamped work off a channel, one request fails, and a lock never releases.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: main
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// A real Go service, traced. Goroutines interleave, a worker claims stamped
// work off a channel, one request fails, and a lock never releases.
package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"time"

	"execviz"
)

func fetchUser(ctx context.Context, uid int) error {
	return execviz.Do(ctx, fmt.Sprintf("fetch_user_%d", uid), "call", func(c context.Context) error {
		execviz.Log(c, "info", fmt.Sprintf("loading user %d", uid))
		return execviz.Do(c, "db_user", "io", func(context.Context) error {
			time.Sleep(time.Duration(40+uid*15) * time.Millisecond)
			return nil
		})
	}, execviz.WithDomain("users"))
}

func fetchOrders(ctx context.Context, uid int) error {
	return execviz.Do(ctx, fmt.Sprintf("fetch_orders_%d", uid), "call", func(c context.Context) error {
		return execviz.Do(c, "db_orders", "io", func(c2 context.Context) error {
			time.Sleep(60 * time.Millisecond)
			if uid == 2 {
				execviz.Log(c2, "error", "order store unavailable")
				return errors.New("order store unavailable")
			}
			return nil
		})
	}, execviz.WithDomain("orders"))
}

func render(ctx context.Context, uid int) error {
	return execviz.Do(ctx, fmt.Sprintf("render_%d", uid), "call", func(c context.Context) error {
		execviz.Log(c, "info", "rendering")
		time.Sleep(20 * time.Millisecond)
		return nil
	}, execviz.WithDomain("render"))
}

func main() {
	execviz.Install(execviz.Config{HostID: "go-1", Domain: "api", FlushMS: 300})
	root := context.Background()
	ctx, rootID := execviz.Start(root, "service", "call")

	// a lock that is never released: an unfinished span, left open on purpose
	_, stuck := execviz.Start(ctx, "reconcile_lock", "wait", execviz.WithDomain("billing"))
	execviz.Lifecycle(stuck, "suspended", nil)

	jobs := make(chan execviz.Stamped[string], 8)
	done := make(chan struct{})
	go func() {
		wctx, _ := execviz.Start(ctx, "worker_loop", "spawn", execviz.WithDomain("worker"))
		for m := range jobs {
			c, item, qid := execviz.Claim(wctx, m)
			_ = execviz.Do(c, "process_"+item, "call", func(c2 context.Context) error {
				execviz.Log(c2, "info", "processing "+item)
				time.Sleep(30 * time.Millisecond)
				return nil
			})
			execviz.Release(qid)
		}
		close(done)
	}()

	failed := 0
	for uid := 0; uid < 3; uid++ {
		uid := uid
		err := execviz.Do(ctx, fmt.Sprintf("GET /profile/%d", uid), "call", func(c context.Context) error {
			if e := execviz.Gather(c, "profile_fanin",
				func(c2 context.Context) error { return fetchUser(c2, uid) },
				func(c2 context.Context) error { return fetchOrders(c2, uid) },
			); e != nil {
				return e
			}
			if e := render(c, uid); e != nil { return e }
			m, _ := execviz.Send(c, "enqueue_job", fmt.Sprintf("invoice-%d", uid))
			jobs <- m
			return nil
		})
		if err != nil { failed++ }
	}
	close(jobs)
	<-done

	execviz.End(rootID, nil)
	execviz.Shutdown()
	fmt.Fprintf(os.Stderr, "go workload complete, %d failed\n", failed)
}
