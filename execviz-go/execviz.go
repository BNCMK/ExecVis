// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz.go
//  script_path: execviz-go/execviz.go
//  module_name: execviz
//  version: 0.53.1
//  description: Package execviz is the execviz capture adapter for Go.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: execviz, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// Package execviz is the execviz capture adapter for Go.
//
// The carrier is context.Context, which is the runtime's own mechanism for
// carrying request-scoped state across goroutine boundaries. A span placed in a
// context stays the parent for anything that context reaches, including work
// started on another goroutine, which makes causality survive
// concurrency here.
//
// Coverage: Go has no per-call hook comparable to a Python profile function, so
// this adapter records explicitly instrumented work plus the context
// propagation around it. That is a property of the runtime and is stated rather
// than hidden.
package execviz

import (
	"strings"
	"io"
	"sort"
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"sync"
	"time"
)

type ctxKey struct{}

type Span struct {
	SpanID    string                 `json:"span_id"`
	TraceID   string                 `json:"trace_id"`
	ParentID  *string                `json:"parent_span_id"`
	Links     []string               `json:"links"`
	Name      string                 `json:"name"`
	Kind      string                 `json:"kind"`
	Start     float64                `json:"start"`
	End       *float64               `json:"end"`
	Status    string                 `json:"status"`
	Lifecycle []map[string]any       `json:"lifecycle"`
	Origin    string                 `json:"origin"`
	// which clock stamped this, so skew analysis knows what it compares
	ClockSource string `json:"clock_source"`
	HostID    string                 `json:"host_id"`
	Domain    string                 `json:"domain"`
	Attrs     map[string]any         `json:"attributes"`
	Events    []map[string]any       `json:"events"`
}

type Config struct {
	Collector string
	HostID    string
	Domain    string
	FlushMS   int
}

var (
	mu      sync.Mutex
	pending = map[string]*Span{}

	// maxPending bounds what is held while delivery is failing.
	//
	// An unreachable collector otherwise means unbounded growth inside the
	// program being observed; a tracing tool that eventually kills the process
	// it is watching, which is the worst failure this design could have.
	maxPending = 20000
	dropped         int64
	droppedTraces   int64
	droppedAbnormal int64
	sent    = map[string]string{}
	cfg     = Config{Collector: "http://127.0.0.1:8900", HostID: "go", Domain: "app", FlushMS: 500}
	traceID string
	stopCh  chan struct{}
)

func sid() string { b := make([]byte, 6); rand.Read(b); return hex.EncodeToString(b) }
func now() float64 { return float64(time.Now().UnixNano()) / 1e9 }

// Install starts the adapter and its flush loop.
func Install(c Config) {
	if c.Collector != "" { cfg.Collector = c.Collector }
	if c.HostID != "" { cfg.HostID = c.HostID }
	if c.Domain != "" { cfg.Domain = c.Domain }
	if c.FlushMS > 0 { cfg.FlushMS = c.FlushMS }
	if h := os.Getenv("EXECVIZ_COLLECTOR"); h != "" { cfg.Collector = h }
	traceID = sid()
	stopCh = make(chan struct{})
	go func() {
		t := time.NewTicker(time.Duration(cfg.FlushMS) * time.Millisecond)
		defer t.Stop()
		for {
			select {
			case <-t.C:
				Flush()
			case <-stopCh:
				return
			}
		}
	}()
}

// Shutdown flushes what is pending and stops the loop.
func Shutdown() { Flush(); if stopCh != nil { close(stopCh); stopCh = nil } }

func current(ctx context.Context) *string {
	if ctx == nil { return nil }
	if v, ok := ctx.Value(ctxKey{}).(string); ok { return &v }
	return nil
}

// Start opens a span and returns a context carrying it, so anything reached
// through that context is attributed to it. Phase one is recorded immediately:
// a process that dies mid-span still reports the span as open.
func Start(ctx context.Context, name, kind string, opts ...Option) (context.Context, string) {
	o := options{domain: cfg.Domain}
	for _, f := range opts { f(&o) }
	id := sid()
	parent := current(ctx)
	if o.parent != nil { parent = o.parent }
	attrs := map[string]any{}
	for k, v := range o.attrs { attrs[k] = v }
	mu.Lock()
	evict()
	pending[id] = &Span{SpanID: id, ClockSource: "time.Now", TraceID: traceID, ParentID: parent, Links: o.links,
		Name: name, Kind: kind, Start: now(), Status: "running",
		Lifecycle: []map[string]any{}, Origin: "semantic", HostID: cfg.HostID,
		Domain: o.domain, Attrs: attrs, Events: []map[string]any{}}
	mu.Unlock()
	if o.links == nil {
		mu.Lock(); pending[id].Links = []string{}; mu.Unlock()
	}
	return context.WithValue(ctx, ctxKey{}, id), id
}

// End writes phase two. The collector upserts on span id, so a completed span
// updates its open row rather than duplicating it.
func End(id string, err error) {
	mu.Lock()
	defer mu.Unlock()
	s, ok := pending[id]
	if !ok { return }
	t := now()
	s.End = &t
	if err != nil {
		s.Status = "error"
		s.Attrs["error"] = err.Error()
	} else {
		s.Status = "ok"
	}
}

func Lifecycle(id, kind string, context map[string]any) {
	mu.Lock()
	defer mu.Unlock()
	if s, ok := pending[id]; ok {
		e := map[string]any{"t": now(), "type": kind}
		if context != nil { e["context"] = context }
		s.Lifecycle = append(s.Lifecycle, e)
	}
}

// Log attaches a line to the span the context carries.
func Log(ctx context.Context, level, msg string) {
	id := current(ctx)
	if id == nil { return }
	mu.Lock()
	defer mu.Unlock()
	if s, ok := pending[*id]; ok {
		s.Events = append(s.Events, map[string]any{"t": now(), "level": level, "msg": msg})
	}
}

type options struct {
	parent *string
	links  []string
	domain string
	attrs  map[string]any
}
type Option func(*options)

func WithParent(id string) Option    { return func(o *options) { o.parent = &id } }
func WithLinks(ids ...string) Option { return func(o *options) { o.links = ids } }
func WithDomain(d string) Option     { return func(o *options) { o.domain = d } }
func WithAttrs(a map[string]any) Option { return func(o *options) { o.attrs = a } }

// Do runs fn inside a span, which is the shape most call sites want.
func Do(ctx context.Context, name, kind string, fn func(context.Context) error, opts ...Option) error {
	c, id := Start(ctx, name, kind, opts...)
	err := fn(c)
	End(id, err)
	return err
}

// Go starts a goroutine. Scheduling work is a crossing, and the child inherits
// its creator because the context travels with it.
func Go(ctx context.Context, name string, fn func(context.Context) error) <-chan error {
	c, id := Start(ctx, name, "spawn")
	ch := make(chan error, 1)
	go func() {
		err := fn(c)
		End(id, err)
		ch <- err
		close(ch)
	}()
	return ch
}

// Gather runs several children and records the continuation as a fan-in: the
// join keeps the enclosing scope as its parent and lists every child in links
//, because parenting a join to one of its children would place it
// outside its parent in time.
func Gather(ctx context.Context, name string, fns ...func(context.Context) error) error {
	parent := current(ctx)
	var wg sync.WaitGroup
	ids := make([]string, len(fns))
	errs := make([]error, len(fns))
	for i, fn := range fns {
		c, id := Start(ctx, fmt.Sprintf("%s[%d]", name, i), "call")
		ids[i] = id
		wg.Add(1)
		go func(i int, c context.Context, id string, fn func(context.Context) error) {
			defer wg.Done()
			err := fn(c)
			errs[i] = err
			End(id, err)
		}(i, c, id, fn)
	}
	wg.Wait()
	var opts []Option
	if parent != nil { opts = append(opts, WithParent(*parent)) }
	opts = append(opts, WithLinks(ids...))
	_, join := Start(ctx, name+"_join", "call", opts...)
	End(join, nil)
	for _, e := range errs { if e != nil { return e } }
	return nil
}

// Stamped is a message carrying the context of the send. Causality is preserved
// at the moment it exists rather than reconstructed on the far side.
type Stamped[T any] struct {
	Item     T
	TraceID  string
	SpanID   string
}

// Send opens a queue span and stamps it onto the value being handed over.
func Send[T any](ctx context.Context, name string, item T) (Stamped[T], string) {
	_, id := Start(ctx, name, "queue")
	return Stamped[T]{Item: item, TraceID: traceID, SpanID: id}, id
}

// Claim reads the stamp back on the receiving side.
func Claim[T any](ctx context.Context, m Stamped[T]) (context.Context, T, string) {
	Lifecycle(m.SpanID, "claimed", map[string]any{"host": cfg.HostID})
	if r := current(ctx); r != nil {
		mu.Lock()
		if s, ok := pending[*r]; ok { s.Links = append(s.Links, m.SpanID) }
		mu.Unlock()
	}
	return context.WithValue(ctx, ctxKey{}, m.SpanID), m.Item, m.SpanID
}

func Release(id string) {
	Lifecycle(id, "released", map[string]any{"host": cfg.HostID})
	End(id, nil)
}

// Flush pushes what has changed. A span is re-sent once its second phase lands
// so completion updates the collector's row; a failed flush is retried.
func Flush() int {
	mu.Lock()
	batch := []*Span{}
	for id, s := range pending {
		state := fmt.Sprintf("%v|%s", s.End != nil, s.Status)
		if sent[id] != state { batch = append(batch, s) }
	}
	mu.Unlock()
	if len(batch) == 0 { return 0 }
	body, _ := json.Marshal(map[string]any{"host_id": cfg.HostID, "spans": batch})
	resp, err := http.Post(cfg.Collector+"/api/ingest", "application/json", bytes.NewReader(body))
	if err != nil { return 0 }
	reply, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != 200 {
		return 0
	}
	reportRefusals(reply)
	mu.Lock()
	for _, s := range batch {
		sent[s.SpanID] = fmt.Sprintf("%v|%s", s.End != nil, s.Status)
		if s.End != nil { delete(pending, s.SpanID) }
	}
	mu.Unlock()
	return len(batch)
}

// evict drops whole traces when the buffer is full, never individual spans.
//
// Two invariants the specification states and the core's retention already
// honours (the specification, the specification):
//
// Trace-level only: dropping a span whose siblings remain punches a hole in that
// trace's graph, leaving a parent pointing at a child that no longer exists. The
// unit of loss is the trace, so what survives is causally complete.
//
// Bias toward the abnormal: a trace holding an error or a still-running span is
// never dropped while an ordinary one remains. Those are the traces someone came
// looking for.
//
// The caller already holds mu.
func evict() {
	if len(pending) <= maxPending {
		return
	}
	type traceInfo struct {
		ids  []string
		last float64
		keep bool
	}
	traces := map[string]*traceInfo{}
	for id, s := range pending {
		t := s.TraceID
		if t == "" {
			t = id
		}
		ti := traces[t]
		if ti == nil {
			ti = &traceInfo{}
			traces[t] = ti
		}
		ti.ids = append(ti.ids, id)
		at := s.Start
		if s.End != nil && *s.End > at {
			at = *s.End
		}
		if at > ti.last {
			ti.last = at
		}
		if s.End == nil || s.Status == "error" {
			ti.keep = true
		}
	}
	type row struct {
		id   string
		last float64
		keep bool
	}
	rows := make([]row, 0, len(traces))
	for t, ti := range traces {
		rows = append(rows, row{t, ti.last, ti.keep})
	}
	sort.Slice(rows, func(i, j int) bool {
		if rows[i].keep != rows[j].keep {
			return !rows[i].keep // ordinary traces are sacrificed first
		}
		return rows[i].last < rows[j].last
	})
	for _, r := range rows {
		if len(pending) <= maxPending {
			break
		}
		for _, id := range traces[r.id].ids {
			delete(pending, id)
			dropped++
		}
		droppedTraces++
		if r.keep {
			droppedAbnormal++
		}
	}
}

// reportRefusals reads what the collector said about the batch.
//
// It names every span it refused and why. That explanation reached nobody while
// the reply was discarded and any 200 treated as complete success, so an adapter
// emitting malformed spans went on emitting them with nothing to show its
// author.
//
// Reported once per distinct reason: a bug in an adapter repeats every second,
// and a message that repeats with it is one nobody reads.
var (
	reportedRefusals = map[string]bool{}
	refusedCount     int64
)

// RefusedByCollector is how many spans this process has had refused.
func RefusedByCollector() int64 {
	mu.Lock()
	defer mu.Unlock()
	return refusedCount
}

func reportRefusals(reply []byte) {
	var r struct {
		Rejected int      `json:"rejected"`
		Reasons  []string `json:"reasons"`
	}
	if err := json.Unmarshal(reply, &r); err != nil || r.Rejected == 0 {
		return // an unreadable reply must never break delivery
	}
	mu.Lock()
	defer mu.Unlock()
	refusedCount += int64(r.Rejected)
	for _, reason := range r.Reasons {
		// the span id changes every time, so key on the explanation itself
		key := reason
		if i := strings.Index(reason, ":"); i >= 0 {
			key = strings.TrimSpace(reason[i+1:])
		}
		if reportedRefusals[key] {
			continue
		}
		reportedRefusals[key] = true
		fmt.Fprintf(os.Stderr,
			"execviz: the collector refused a span; %s\n"+
				"  (further spans refused for this reason will not be reported again)\n",
			reason)
	}
}
