// ========================================================================
//  MANIFEST
// ========================================================================
//  script_name: workload.mjs
//  script_path: execviz-node/workload.mjs
//  module_name: workload
//  version: 0.53.1
//  description: A real Node service, traced. Concurrent requests interleave, a worker claims stamped work off a queue, one request fails, and a watchdog never returns.
//  kind: module
//  spec: internal
//  internal_dependencies: execviz.mjs
//  external_dependencies: 
//  features: workload
//  api_version: execvis-v1.0.0
//  last_updated: 2026-08-07
// ========================================================================

// A real Node service, traced. Concurrent requests interleave, a worker claims
// stamped work off a queue, one request fails, and a watchdog never returns.
import * as ev from './execviz.mjs';

ev.install({
  collector: process.env.EXECVIZ_COLLECTOR || 'http://127.0.0.1:8930',
  hostId: 'node-1',
  domain: 'api',
  flushMs: 400,
});

const sleep = (ms) => new Promise(r => setTimeout(r, ms));

// ========================================================================
// INTERNALS
// ========================================================================

// A closable queue. A consumer waits on a promise rather than polling, and
// close() wakes every waiter, so a producer that fails and never enqueues
// cannot strand the consumer. The previous version counted expected jobs and
// deadlocked the moment a request failed before pushing one.
function makeQueue() {
  const items = [];
  let waiters = [];
  let closed = false;
  return {
    push(v) {
      if (closed) return;
      items.push(v);
      const w = waiters.shift();
      if (w) w(items.shift());
    },
    // Resolves with an item, or with null once the queue is closed and drained.
    take() {
      if (items.length) return Promise.resolve(items.shift());
      if (closed) return Promise.resolve(null);
      return new Promise(resolve => waiters.push(resolve));
    },
    close() {
      closed = true;
      for (const w of waiters) w(items.length ? items.shift() : null);
      waiters = [];
    },
    get size() { return items.length; },
  };
}
const queue = makeQueue();

async function fetchUser(uid) {
  ev.setDomain('users');
  return ev.withSpan(`fetch_user_${uid}`, 'call', async () => {
    await ev.awaitSpan(sleep(40 + uid * 15), 'db_user', 'io');
    return { id: uid };
  });
}

async function fetchOrders(uid) {
  ev.setDomain('orders');
  return ev.withSpan(`fetch_orders_${uid}`, 'call', async () => {
    await ev.awaitSpan(sleep(60), 'db_orders', 'io');
    if (uid === 2) throw new Error('order store unavailable');
    return [1, 2];
  });
}

async function render(uid) {
  ev.setDomain('render');
  return ev.withSpan(`render_${uid}`, 'call', async () => {
    await ev.awaitSpan(sleep(20), 'template', 'wait');
  });
}

async function handle(uid) {
  ev.setDomain('api');
  return ev.withSpan(`GET /profile/${uid}`, 'call', async () => {
    try {
      await ev.gatherSpan('profile_fanin', [() => fetchUser(uid), () => fetchOrders(uid)]);
    } catch (e) {
      const err = ev.spanStart('order_lookup_failed', 'error');
      ev.spanEnd(err, 'error', { message: String(e.message) });
      throw e;
    }
    await render(uid);
    queue.push(ev.stamp({ job: `invoice-${uid}` }));   // stamped crossing
  });
}

async function worker() {
  ev.setDomain('worker');
  return ev.spawn(async () => {
    // Blocks on the queue and exits when it closes. No job count, no polling,
    // no shared flag: termination is a property of the queue.
    for (;;) {
      const msg = await queue.take();
      if (msg === null) return;
      const { item, spanId } = ev.claim(msg);
      await ev.withSpan(`process_${item.job}`, 'call', async () => {
        await ev.awaitSpan(sleep(30), 'write_invoice', 'io');
      });
      ev.release(spanId);
    }
  }, 'worker_loop');
}

async function main() {
  await ev.withSpan('service', 'call', async () => {
    ev.setDomain('billing');
    const stuck = ev.spanStart('reconcile_lock', 'wait');   // never completes
    ev.spanLifecycle(stuck, 'suspended');
    ev.setDomain('api');

    const w = worker();
    const results = await Promise.allSettled([0, 1, 2].map(handle));
    queue.close();          // wakes the worker whether or not anything arrived
    await w;
    const failed = results.filter(r => r.status === 'rejected').length;
    console.log(`requests done, ${failed} failed`);
  });
  await ev.flush();
  await ev.shutdown();
}

main().then(() => console.log('node workload complete'));
