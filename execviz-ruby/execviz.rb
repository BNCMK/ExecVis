# =========================================================================
#  MANIFEST
# =========================================================================
#  script_name: execviz.rb
#  script_path: execviz-ruby/execviz.rb
#  module_name: execviz
#  version: 0.53.1
#  description: frozen_string_literal: true
#  kind: module
#  spec: internal
#  internal_dependencies: 
#  external_dependencies: 
#  features: execviz
#  api_version: execvis-v1.0.0
#  last_updated: 2026-08-07
# =========================================================================

# frozen_string_literal: true

# execviz capture adapter for Ruby.
#
# The carrier is Fiber storage, which Ruby scopes to the fiber and inherits into
# a fiber created inside it. That is the closest thing the runtime offers to an
# automatic carrier, and it is what keeps the parent link correct when work is
# suspended and resumed. A Thread does not inherit it, because a thread may
# outlive and predate the work it runs, so `Execviz.thread` carries it across
# explicitly rather than pretending otherwise.
#
# Coverage: Ruby exposes TracePoint, which can see every call, and that is the
# breadth option. It is off by default here because it is expensive enough to
# distort what it measures; `Execviz.autotrace!` turns it on when breadth is
# worth more than fidelity.
require 'json'
require 'net/http'
require 'securerandom'
require 'socket'
require 'uri'

module Execviz
  Config = Struct.new(:collector, :host_id, :domain, :flush_secs, keyword_init: true)

  @config = Config.new(
    collector: ENV.fetch('EXECVIZ_COLLECTOR', 'http://127.0.0.1:8900'),
    host_id: ENV.fetch('EXECVIZ_HOST', Socket.gethostname),
    domain: 'app', flush_secs: 0.5
  )
  @pending = {}
  # The most spans held while delivery is failing.
  #
  # An unreachable collector otherwise means unbounded growth inside the program
  # being observed; a tracing tool that eventually kills the process it is
  # watching, which is the worst failure this design could have.
  MAX_PENDING = 20_000
  @dropped = 0
  @dropped_traces = 0
  @dropped_abnormal = 0
  @refused_by_collector = 0
  @reported_refusals = []
  @sent = {}
  @mutex = Mutex.new
  @trace_id = nil
  @flusher = nil
  @tracepoint = nil

  class << self
    attr_reader :config

    def install(collector: nil, host_id: nil, domain: nil, flush_secs: nil)
      @config.collector = collector if collector
      @config.host_id = host_id if host_id
      @config.domain = domain if domain
      @config.flush_secs = flush_secs if flush_secs
      @trace_id = sid
      @flusher = Thread.new do
        loop do
          sleep @config.flush_secs
          begin; flush; rescue StandardError; end
        end
      end
      @flusher.abort_on_exception = false
      @config
    end

    def shutdown
      autotrace_off!
      flush
      @flusher&.kill
      @flusher = nil
    end

    def set_domain(d) = @config.domain = d
    def current_span = Fiber[:execviz_span]

    # Phase one, recorded the moment the span opens, so a process that dies
    # mid-span still reports it as open rather than not at all.
    def span_start(name, kind = 'call', parent: nil, links: nil, domain: nil, attributes: nil)
      id = sid
      @mutex.synchronize do
        # reported, so the counter starts again rather than being resent forever
        @dropped = 0
        @dropped_traces = 0
        @dropped_abnormal = 0
        @pending[id] = {
          'span_id' => id, 'trace_id' => @trace_id,
          'parent_span_id' => parent || current_span,
          'links' => links || [], 'name' => name, 'kind' => kind,
          'start' => now, 'end' => nil, 'status' => 'running',
          'lifecycle' => [], 'events' => [], 'origin' => 'semantic',
          # which clock stamped this, so skew analysis knows what it compares
          'clock_source' => 'Process.clock_gettime',
          'host_id' => @config.host_id, 'domain' => domain || @config.domain,
          'attributes' => (attributes || {}).merge('thread' => Thread.current.name.to_s)
        }
        evict
      end
      id
    end

    # Phase two. The far end upserts on span id, so completion updates the open
    # row rather than duplicating it.
    def span_end(id, status = 'ok', attributes = nil)
      @mutex.synchronize do
        s = @pending[id] or next
        s['end'] = now
        s['status'] = status
        s['attributes'].merge!(attributes) if attributes
      end
    end

    def lifecycle(id, type, context = nil)
      @mutex.synchronize do
        s = @pending[id] or next
        e = { 't' => now, 'type' => type }
        e['context'] = context if context
        s['lifecycle'] << e
      end
    end

    # A log line belongs to the span that was running when it was written (2.6).
    def log(level, msg)
      id = current_span or return
      @mutex.synchronize do
        s = @pending[id] or next
        s['events'] << { 't' => now, 'level' => level.to_s, 'msg' => msg.to_s }
      end
    end

    # Runs the block inside a span, with the span active for anything it reaches.
    def span(name, kind = 'call', domain: nil, links: nil)
      id = span_start(name, kind, domain: domain, links: links)
      prev = Fiber[:execviz_span]
      Fiber[:execviz_span] = id
      begin
        r = yield id
        span_end(id, 'ok')
        r
      rescue StandardError => e
        span_end(id, 'error', { 'error' => e.message })
        raise
      ensure
        Fiber[:execviz_span] = prev
      end
    end

    # A thread does not inherit fiber storage, so the span is carried across the
    # boundary explicitly. Starting work is a crossing.
    def thread(name = 'thread', &blk)
      parent = current_span
      id = span_start(name, 'spawn', parent: parent)
      Thread.new do
        Fiber[:execviz_span] = id
        begin
          blk.call
          span_end(id, 'ok')
        rescue StandardError => e
          span_end(id, 'error', { 'error' => e.message })
        end
      end
    end

    # A fan-in: the join keeps the enclosing scope as its parent and lists every
    # child in links (3.0a), because parenting it to a child would place it
    # outside its parent in time.
    def gather(name, blocks)
      parent = current_span
      ids = []
      threads = blocks.each_with_index.map do |blk, i|
        id = span_start("#{name}[#{i}]", 'call', parent: parent)
        ids << id
        Thread.new do
          Fiber[:execviz_span] = id
          begin
            blk.call
            span_end(id, 'ok')
          rescue StandardError => e
            span_end(id, 'error', { 'error' => e.message })
            Thread.current[:execviz_error] = e
          end
        end
      end
      threads.each(&:join)
      join = span_start("#{name}_join", 'call', parent: parent, links: ids)
      span_end(join, 'ok')
      err = threads.map { |t| t[:execviz_error] }.compact.first
      raise err if err

      nil
    end

    # Context stamped onto a value crossing a boundary, read back on the far side.
    def stamp(item)
      { 'item' => item, 'trace_id' => @trace_id, 'span' => current_span }
    end

    def claim(msg)
      qs = msg['span'] or return [msg['item'], nil]
      lifecycle(qs, 'claimed', { 'thread' => Thread.current.name.to_s })
      receiver = current_span
      if receiver
        @mutex.synchronize do
          s = @pending[receiver]
          s['links'] << qs if s && !s['links'].include?(qs)
        end
      end
      Fiber[:execviz_span] = qs
      [msg['item'], qs]
    end

    def release(qs)
      return unless qs

      lifecycle(qs, 'released', { 'thread' => Thread.current.name.to_s })
      span_end(qs, 'ok')
    end

    # Breadth on demand: TracePoint sees every call, at a cost high enough to
    # distort the measurement, so it is opt-in rather than the default.
    def autotrace!(only: nil)
      @tracepoint = TracePoint.new(:call, :return) do |tp|
        next if tp.path.include?('execviz.rb')
        next if only && !tp.path.include?(only)

        if tp.event == :call
          id = span_start(tp.method_id.to_s, 'call', domain: File.basename(tp.path, '.rb'))
          (Fiber[:execviz_auto] ||= []) << id
          Fiber[:execviz_span] = id
        else
          stack = Fiber[:execviz_auto]
          id = stack&.pop or next
          span_end(id, 'ok')
          Fiber[:execviz_span] = stack.last
        end
      end
      @tracepoint.enable
    end

    def autotrace_off! = @tracepoint&.disable

    # Delivery: push straight to an execviz instance. A span is re-sent once its
    # second phase lands; a failed flush is retried on the next tick.
    # Drops whole traces when the buffer is full, never individual spans.
    #
    # Two invariants the specification states and the core's retention already
    # honours (the specification, the specification). Trace-level only: dropping a span whose siblings
    # remain punches a hole in that trace's graph. And bias toward the abnormal:
    # a trace holding an error or a still-running span is never dropped while an
    # ordinary one remains, because those are the traces someone came looking
    # for.
    #
    # The caller already holds @mutex.
    def evict
      return if @pending.size <= MAX_PENDING

      traces = Hash.new { |h, k| h[k] = { ids: [], last: 0.0, keep: false } }
      @pending.each do |id, s|
        t = s['trace_id'] || id
        rec = traces[t]
        rec[:ids] << id
        rec[:last] = [rec[:last], (s['end'] || s['start']).to_f].max
        rec[:keep] = true if s['end'].nil? || s['status'] == 'error'
      end

      order = traces.map { |t, rec| [t, rec[:keep] ? 1 : 0, rec[:last]] }
      order.sort_by! { |(_, keep, last)| [keep, last] }
      order.each do |(t, keep, _)|
        break if @pending.size <= MAX_PENDING

        traces[t][:ids].each do |id|
          @pending.delete(id)
          @dropped += 1
        end
        @dropped_traces += 1
        @dropped_abnormal += 1 if keep == 1
      end
    end

    # Reads what the collector said about the batch.
    #
    # It names every span it refused and why. That explanation reached nobody
    # while the reply was discarded and any 200 treated as complete success, so
    # an adapter emitting malformed spans went on emitting them with nothing to
    # show its author.
    #
    # Reported once per distinct reason: a bug in an adapter repeats every
    # second, and a message that repeats with it is one nobody reads.
    def report_refusals(body)
      reply = begin
        JSON.parse(body.to_s)
      rescue StandardError
        return   # an unreadable reply must never break delivery
      end
      return unless reply.is_a?(Hash) && reply['rejected'].to_i.positive?

      @refused_by_collector += reply['rejected'].to_i
      Array(reply['reasons']).each do |reason|
        # the span id changes every time, so key on the explanation itself
        key = reason.include?(':') ? reason.split(':', 2).last.strip : reason
        next if @reported_refusals.include?(key)

        @reported_refusals << key
        warn "execviz: the collector refused a span; #{reason}\n" \
             "  (further spans refused for this reason will not be reported again)"
      end
    end

    def flush
      batch = @mutex.synchronize do
        @pending.values.select { |s| @sent[s['span_id']] != state_of(s) }.map(&:dup)
      end
      return 0 if batch.empty?

      uri = URI("#{@config.collector.chomp('/')}/api/ingest")
      begin
        payload = { 'host_id' => @config.host_id, 'spans' => batch }
        if @dropped.to_i.positive?
          # the collector is told the record is incomplete, and how badly
          payload['dropped'] = @dropped
          payload['dropped_traces'] = @dropped_traces
          payload['dropped_abnormal'] = @dropped_abnormal
        end
        res = Net::HTTP.post(uri, JSON.generate(payload),
                             'Content-Type' => 'application/json')
        return 0 unless res.code.to_i == 200

        report_refusals(res.body)
      rescue StandardError
        return 0
      end
      @mutex.synchronize do
        batch.each do |s|
          @sent[s['span_id']] = state_of(s)
          @pending.delete(s['span_id']) if s['end']
        end
      end
      batch.size
    end

    private

    def state_of(s) = "#{!s['end'].nil?}|#{s['status']}"
    def sid = SecureRandom.hex(6)
    def now = Time.now.to_f
  end
end
