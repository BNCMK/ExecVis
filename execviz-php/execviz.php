<?php
// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: execviz.php
//  script_path: execviz-php/execviz.php
//  module_name: execviz
//  version: 0.53.1
//  description: execviz capture adapter for PHP.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: execviz, capture, adapter
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

declare(strict_types=1);

/**
 * execviz capture adapter for PHP.
 *
 * PHP's carrier problem is the opposite of everyone else's. A classic request
 * is one synchronous flow in one process, so a plain stack is a valid parent
 * chain, because this adapter keeps one. Fibers change that: a fiber
 * suspends and resumes with its own stack, so a span opened inside one belongs
 * to that fiber and not to whatever resumed it. Each fiber therefore gets its
 * own stack, keyed by the fiber object, and the main flow keeps its own.
 *
 * Coverage: without an extension PHP offers no per-call hook, so this records
 * explicitly instrumented work. Declared rather than glossed over.
 */
final class Execviz
{
    private static ?Execviz $instance = null;

    private string $collector;
    private string $hostId;
    private string $domain;
    private string $traceId;
    /** @var array<string, array<string,mixed>> */
    private array $pending = [];

    /** The most spans held while delivery is failing.
     *
     * An unreachable collector otherwise means unbounded growth inside the
     * program being observed; a tracing tool that eventually kills the process
     * it is watching, which is the worst failure this design could have.
     */
    private const MAX_PENDING = 20000;
    private int $dropped = 0;
    private int $droppedTraces = 0;
    private int $droppedAbnormal = 0;
    private int $refusedByCollector = 0;
    /** @var array<string,bool> one message per distinct reason, not per batch */
    private array $reportedRefusals = [];
    /** @var array<string, string> */
    private array $sent = [];
    /** @var array<string, list<string>> one stack per fiber, plus the main flow */
    private array $stacks = ['main' => []];
    private int $batch;

    private function __construct(string $collector, string $hostId, string $domain, int $batch)
    {
        $this->collector = rtrim($collector, '/');
        $this->hostId = $hostId;
        $this->domain = $domain;
        $this->batch = $batch;
        $this->traceId = self::sid();
    }

    public static function install(
        ?string $collector = null,
        string $hostId = 'php',
        string $domain = 'app',
        int $batch = 200
    ): Execviz {
        $collector ??= getenv('EXECVIZ_COLLECTOR') ?: 'http://127.0.0.1:8900';
        self::$instance = new self($collector, $hostId, $domain, $batch);
        // A request that dies still reports what it was doing: the shutdown
        // handler delivers whatever is open, and open spans stay open.
        register_shutdown_function(static function (): void {
            self::$instance?->flush();
        });
        return self::$instance;
    }

    public static function i(): Execviz
    {
        return self::$instance ?? self::install();
    }

    public function setDomain(string $d): void
    {
        $this->domain = $d;
    }

    /** A fiber has its own stack because it has its own suspension. */
    private function stackKey(): string
    {
        $f = Fiber::getCurrent();
        return $f === null ? 'main' : 'fiber:' . spl_object_id($f);
    }

    /**
     * Makes a span current for the rest of this request.
     *
     * `span()` scopes a span to a callable, which is right for a unit of work
     * and wrong for a whole request: attaching with no source change needs one
     * span covering the run, and there is no callable to wrap it in.
     */
    public function makeCurrent(string $id): void
    {
        $k = $this->stackKey();
        $this->stacks[$k][] = $id;
    }

    public function currentSpan(): ?string
    {
        $k = $this->stackKey();
        $s = $this->stacks[$k] ?? [];
        return $s === [] ? null : $s[count($s) - 1];
    }

    /**
     * Phase one, written the moment the span opens.
     * @param list<string> $links
     * @param array<string,mixed> $attributes
     */
    public function spanStart(
        string $name,
        string $kind = 'call',
        ?string $parent = null,
        array $links = [],
        ?string $domain = null,
        array $attributes = []
    ): string {
        $id = self::sid();
        $this->pending[$id] = [
            'span_id' => $id,
            'trace_id' => $this->traceId,
            'parent_span_id' => $parent ?? $this->currentSpan(),
            'links' => $links,
            'name' => $name,
            'kind' => $kind,
            'start' => microtime(true),
            'end' => null,
            'status' => 'running',
            'lifecycle' => [],
            'events' => [],
            'origin' => 'semantic',
            // which clock stamped this, so skew analysis knows what it compares
            'clock_source' => 'microtime',
            'host_id' => $this->hostId,
            'domain' => $domain ?? $this->domain,
            'attributes' => $attributes,
        ];
        $this->evict();
        return $id;
    }

    /** Phase two. The far end upserts, so completion updates rather than duplicates. */
    public function spanEnd(string $id, string $status = 'ok', array $attributes = []): void
    {
        if (!isset($this->pending[$id])) {
            return;
        }
        $this->pending[$id]['end'] = microtime(true);
        $this->pending[$id]['status'] = $status;
        if ($attributes !== []) {
            $this->pending[$id]['attributes'] += $attributes;
        }
        if (count($this->pending) >= $this->batch) {
            $this->flush();
        }
    }

    public function lifecycle(string $id, string $type, ?array $context = null): void
    {
        if (!isset($this->pending[$id])) {
            return;
        }
        $e = ['t' => microtime(true), 'type' => $type];
        if ($context !== null) {
            $e['context'] = $context;
        }
        $this->pending[$id]['lifecycle'][] = $e;
    }

    /** A log line belongs to the span that was running when it was written (2.6). */
    public function log(string $level, string $msg): void
    {
        $id = $this->currentSpan();
        if ($id === null || !isset($this->pending[$id])) {
            return;
        }
        $this->pending[$id]['events'][] = ['t' => microtime(true), 'level' => $level, 'msg' => $msg];
    }

    /** Runs the callable inside a span, with the span active for what it reaches. */
    public function span(string $name, string $kind, callable $fn, ?string $domain = null, array $links = []): mixed
    {
        $id = $this->spanStart($name, $kind, null, $links, $domain);
        $k = $this->stackKey();
        $this->stacks[$k][] = $id;
        try {
            $r = $fn($id);
            $this->spanEnd($id, 'ok');
            return $r;
        } catch (Throwable $e) {
            $this->spanEnd($id, 'error', ['error' => $e->getMessage()]);
            throw $e;
        } finally {
            array_pop($this->stacks[$k]);
        }
    }

    /**
     * A fiber is a crossing: it inherits the span that created it, and gets its
     * own stack so its suspensions do not disturb the caller's.
     */
    public function fiber(string $name, callable $fn): Fiber
    {
        $parent = $this->currentSpan();
        $id = $this->spanStart($name, 'spawn', $parent);
        $fiber = new Fiber(function () use ($fn, $id): void {
            $k = $this->stackKey();
            $this->stacks[$k] = [$id];
            try {
                $fn();
                $this->spanEnd($id, 'ok');
            } catch (Throwable $e) {
                $this->spanEnd($id, 'error', ['error' => $e->getMessage()]);
            } finally {
                unset($this->stacks[$k]);
            }
        });
        return $fiber;
    }

    /**
     * A fan-in: the join keeps the enclosing scope as its parent and lists every
     * child in links (3.0a). Parenting it to a child would place it outside its
     * parent in time.
     *
     * @param list<callable> $fns
     */
    public function gather(string $name, array $fns): void
    {
        $parent = $this->currentSpan();
        $ids = [];
        $err = null;
        foreach ($fns as $i => $fn) {
            $id = $this->spanStart($name . '[' . $i . ']', 'call', $parent);
            $ids[] = $id;
            $k = $this->stackKey();
            $this->stacks[$k][] = $id;
            try {
                $fn();
                $this->spanEnd($id, 'ok');
            } catch (Throwable $e) {
                $this->spanEnd($id, 'error', ['error' => $e->getMessage()]);
                $err ??= $e;
            } finally {
                array_pop($this->stacks[$k]);
            }
        }
        $join = $this->spanStart($name . '_join', 'call', $parent, $ids);
        $this->spanEnd($join, 'ok');
        if ($err !== null) {
            throw $err;
        }
    }

    /** Context stamped onto a value crossing a boundary, read back on the far side. */
    public function stamp(mixed $item): array
    {
        return ['item' => $item, 'trace_id' => $this->traceId, 'span' => $this->currentSpan()];
    }

    public function claim(array $msg): array
    {
        $qs = $msg['span'] ?? null;
        if ($qs === null) {
            return [$msg['item'] ?? null, null];
        }
        $this->lifecycle($qs, 'claimed', ['host' => $this->hostId]);
        $receiver = $this->currentSpan();
        if ($receiver !== null && isset($this->pending[$receiver])
            && !in_array($qs, $this->pending[$receiver]['links'], true)) {
            $this->pending[$receiver]['links'][] = $qs;
        }
        $this->stacks[$this->stackKey()][] = $qs;
        return [$msg['item'] ?? null, $qs];
    }

    public function release(?string $qs): void
    {
        if ($qs === null) {
            return;
        }
        $this->lifecycle($qs, 'released', ['host' => $this->hostId]);
        $this->spanEnd($qs, 'ok');
        $k = $this->stackKey();
        if (($this->stacks[$k] ?? []) !== [] && end($this->stacks[$k]) === $qs) {
            array_pop($this->stacks[$k]);
        }
    }

    /** A span is re-sent once its second phase lands; a failed flush is retried. */
    /**
     * Drops whole traces when the buffer is full, never individual spans.
     *
     * Two invariants the specification states and the core's retention already
     * honours. Trace-level only: dropping a span whose siblings remain punches a
     * hole in that trace's graph. And bias toward the abnormal: a trace holding
     * an error or a still-running span is never dropped while an ordinary one
     * remains, because those are the traces someone came looking for.
     */
    private function evict(): void
    {
        if (count($this->pending) <= self::MAX_PENDING) {
            return;
        }
        $traces = [];
        foreach ($this->pending as $id => $s) {
            $t = $s['trace_id'] ?? $id;
            if (!isset($traces[$t])) {
                $traces[$t] = ['ids' => [], 'last' => 0.0, 'keep' => false];
            }
            $traces[$t]['ids'][] = $id;
            $at = (float)($s['end'] ?? $s['start'] ?? 0.0);
            if ($at > $traces[$t]['last']) {
                $traces[$t]['last'] = $at;
            }
            if ($s['end'] === null || ($s['status'] ?? '') === 'error') {
                $traces[$t]['keep'] = true;
            }
        }
        $order = [];
        foreach ($traces as $t => $rec) {
            $order[] = [$t, $rec['keep'] ? 1 : 0, $rec['last']];
        }
        usort($order, function ($a, $b) {
            if ($a[1] !== $b[1]) {
                return $a[1] <=> $b[1];      // ordinary traces first
            }
            return $a[2] <=> $b[2];          // then oldest
        });
        foreach ($order as [$t, $keep, $_]) {
            if (count($this->pending) <= self::MAX_PENDING) {
                break;
            }
            foreach ($traces[$t]['ids'] as $id) {
                unset($this->pending[$id]);
                $this->dropped++;
            }
            $this->droppedTraces++;
            if ($keep === 1) {
                $this->droppedAbnormal++;
            }
        }
    }

    /**
     * Reads what the collector said about the batch.
     *
     * It names every span it refused and why. That explanation reached nobody
     * while the reply was discarded and any 200 treated as complete success, so
     * an adapter emitting malformed spans went on emitting them with nothing to
     * show its author.
     *
     * Reported once per distinct reason: a bug in an adapter repeats every
     * second, and a message that repeats with it is one nobody reads.
     */
    private function reportRefusals(string $body): void
    {
        $reply = json_decode($body, true);
        if (!is_array($reply) || (int)($reply['rejected'] ?? 0) <= 0) {
            return;   // an unreadable reply must never break delivery
        }
        $this->refusedByCollector += (int)$reply['rejected'];
        foreach (($reply['reasons'] ?? []) as $reason) {
            // the span id changes every time, so key on the explanation itself
            $pos = strpos($reason, ':');
            $key = $pos === false ? $reason : trim(substr($reason, $pos + 1));
            if (isset($this->reportedRefusals[$key])) {
                continue;
            }
            $this->reportedRefusals[$key] = true;
            fwrite(STDERR,
                "execviz: the collector refused a span; {$reason}\n" .
                "  (further spans refused for this reason will not be reported again)\n");
        }
    }

    public function flush(): int
    {
        $batch = [];
        foreach ($this->pending as $id => $s) {
            $state = ($s['end'] !== null ? '1' : '0') . '|' . $s['status'];
            if (($this->sent[$id] ?? null) !== $state) {
                $batch[] = $s;
            }
        }
        if ($batch === []) {
            return 0;
        }
        $payload = ['host_id' => $this->hostId, 'spans' => $batch];
        if ($this->dropped > 0) {
            // the collector is told the record is incomplete, and how badly
            $payload['dropped'] = $this->dropped;
            $payload['dropped_traces'] = $this->droppedTraces;
            $payload['dropped_abnormal'] = $this->droppedAbnormal;
        }
        $body = json_encode($payload, JSON_UNESCAPED_SLASHES);
        $ctx = stream_context_create(['http' => [
            'method' => 'POST',
            'header' => "Content-Type: application/json\r\n",
            'content' => $body,
            'timeout' => 8,
            'ignore_errors' => true,
        ]]);
        $res = @file_get_contents($this->collector . '/api/ingest', false, $ctx);
        if ($res === false) {
            return 0;
        }
        $this->reportRefusals($res);
        // the loss has been reported, so the counter starts again rather than
        // being counted on every later delivery
        $this->dropped = 0;
        $this->droppedTraces = 0;
        $this->droppedAbnormal = 0;
        foreach ($batch as $s) {
            $id = $s['span_id'];
            $this->sent[$id] = ($s['end'] !== null ? '1' : '0') . '|' . $s['status'];
            if ($s['end'] !== null) {
                unset($this->pending[$id]);
            }
        }
        return count($batch);
    }

    private static function sid(): string
    {
        return bin2hex(random_bytes(6));
    }
}
