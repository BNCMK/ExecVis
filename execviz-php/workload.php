<?php
// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: workload.php
//  script_path: execviz-php/workload.php
//  module_name: workload
//  version: 0.53.1
//  description: A real PHP service, traced. Requests fan in, a fiber worker claims stamped work off a queue, one request fails, and a lock never releases.
//  kind: module
//  spec: internal
//  internal_dependencies: 
//  external_dependencies: 
//  features: workload
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

declare(strict_types=1);
require __DIR__ . '/execviz.php';

// A real PHP service, traced. Requests fan in, a fiber worker claims stamped
// work off a queue, one request fails, and a lock never releases.
$ev = Execviz::install(getenv('EXECVIZ_COLLECTOR') ?: null, 'php-1', 'api');

$root = $ev->spanStart('service', 'call');
$stuck = $ev->spanStart('reconcile_lock', 'wait', $root, [], 'billing');
$ev->lifecycle($stuck, 'suspended');

$jobs = [];
$failed = 0;

for ($uid = 0; $uid < 3; $uid++) {
    try {
        $ev->span("GET /profile/$uid", 'call', function () use ($ev, $uid, &$jobs): void {
            $ev->gather('profile_fanin', [
                function () use ($ev, $uid): void {
                    $ev->span("fetch_user_$uid", 'call', function () use ($ev): void {
                        $ev->log('info', 'loading user');
                        $ev->span('db_user', 'io', function (): void { usleep(40000); });
                    }, 'users');
                },
                function () use ($ev, $uid): void {
                    $ev->span("fetch_orders_$uid", 'call', function () use ($ev, $uid): void {
                        $ev->span('db_orders', 'io', function () use ($ev, $uid): void {
                            usleep(60000);
                            if ($uid === 2) {
                                $ev->log('error', 'order store unavailable');
                                throw new RuntimeException('order store unavailable');
                            }
                        });
                    }, 'orders');
                },
            ]);
            $ev->span("render_$uid", 'call', function (): void { usleep(20000); }, 'render');
            $qid = $ev->spanStart('enqueue_job', 'queue');
            $m = $ev->stamp("invoice-$uid");
            $m['span'] = $qid;
            $jobs[] = $m;
        }, 'api');
    } catch (Throwable) {
        $failed++;
    }
}

// the worker runs in a fiber, so it has its own stack and its own suspensions
$worker = $ev->fiber('worker_loop', function () use ($ev, &$jobs): void {
    while ($jobs !== []) {
        $msg = array_shift($jobs);
        [$item, $qid] = $ev->claim($msg);
        $ev->span("process_$item", 'call', function () use ($ev, $item): void {
            $ev->log('info', "processing $item");
            usleep(30000);
        }, 'worker');
        $ev->release($qid);
        Fiber::suspend();
    }
});
$worker->start();
while (!$worker->isTerminated()) {
    $worker->resume();
}

$ev->spanEnd($root, 'ok');
$ev->flush();
fwrite(STDERR, "php workload complete, $failed failed\n");
