<!--
===========================================================================
  MANIFEST
===========================================================================
  script_name: README.md
  script_path: execviz-ruby/README.md
  module_name: README
  version: 0.53.1
  description: execviz capture adapter for Ruby
  kind: document
  spec: internal
  internal_dependencies:
  external_dependencies:
  features: README, capture, adapter
  api_version: execvis-v1.0.0
  last_updated: 2026-08-07
===========================================================================
-->

# execviz capture adapter for Ruby

Reports spans from a Ruby program to a collector, using the same wire
format every other runtime uses.

    ruby workload.rb        # EXECVIZ_COLLECTOR=http://127.0.0.1:8900

    Execviz.install(host_id: 'ruby-1', domain: 'api')

    Execviz.span('GET /profile', 'call') do
      Execviz.gather('fanin', [-> { fetch_user }, -> { fetch_orders }])
      Execviz.span('render', 'call') { render }
    end

    t = Execviz.thread('worker') { worker }      # carries the span across

    jobs << Execviz.stamp(item)                  # stamp the crossing
    item, qid = Execviz.claim(jobs.pop)          # read it back
    Execviz.release(qid)

## The carrier, and where it stops

Fiber storage is scoped to the fiber and inherited by a fiber created inside it,
which is the closest thing the runtime offers to an automatic carrier. A Thread
does not inherit it, because a thread may predate and outlive the work it later
runs, so `Execviz.thread` carries the span across that boundary explicitly rather
than pretending the problem is not there.

## Coverage

Ruby has `TracePoint`, which can see every call, and that is the breadth option.
It is off by default because it is expensive enough to distort what it measures.
`Execviz.autotrace!` turns it on when breadth is worth more than fidelity, with
`only:` to confine it to the code under test.

## Verification

`workload.rb` runs three concurrent requests, one of which fails, a worker
claiming stamped jobs off a queue, and a lock that never releases:

    MISATTRIBUTED PARENTS: 0
    links: profile_fanin_join 2, worker_loop 1, enqueue_job 1
    lifecycle: reconcile_lock suspended, enqueue_job claimed/released
    open spans: reconcile_lock
    conformant: true

Every `fetch_user_N` and `fetch_orders_N` traces back to `GET /profile/N`, though
all six ran on threads started for the fan-in.

## Capturing the logs the program already writes

    Execviz.install(collector: '...', host_id: 'ruby-1')
    Execviz.capture_logs

    Execviz.span('handle_request', 'call') do
      log = Logger.new($stdout)
      log.info('loading user 42')      # captured, no Execviz call
      puts 'a plain puts'
    end

Ruby has no logging registry: `Logger` instances are created wherever a library
wants one and there is no way to enumerate them. What they all share is a
destination, so that is where this attaches; `$stdout` and `$stderr` are teed,
which covers `puts`, `warn`, and every Logger pointed at either. `Warning.warn`
catches Ruby's own warnings and `at_exit` catches the flush on the way out.

**A Logger writing to a file is not captured.** Attaching to a destination
catches what goes to that destination, and that limit is stated rather than
papered over.

**The level is the program's claim, not the stream's.** A `Logger#warn` line
reaching `$stdout` was being recorded as `info`, because a stream only implies a
level while the program stated one. Ruby's default Logger format carries the
severity explicitly, so it is read back out and wins; a line that does not match
that format keeps the level its destination implies, and nothing else is guessed:

    info     'I, [...]  INFO -- : loading user 42'
    warning  'W, [...]  WARN -- : cache miss'
    error    'E, [...] ERROR -- : downstream refused'
    info     'a plain puts'
    error    'a plain stderr line'
