% =========================================================================
%  MANIFEST
% =========================================================================
%  script_name: execviz.erl
%  script_path: execviz-erl/execviz.erl
%  module_name: execviz
%  version: 0.53.1
%  description: %% execviz capture adapter for the BEAM.
%  kind: module
%  spec: internal
%  internal_dependencies: 
%  external_dependencies: 
%  features: execviz, capture, adapter
%  api_version: execvis-v1.0.0
%  last_updated: 2026-08-07
% =========================================================================

%%% execviz capture adapter for the BEAM.
%%%
%%% The BEAM's carrier problem is the cleanest of any runtime supported so far,
%%% and the least forgiving. A process has its own heap and its own process
%%% dictionary, so the dictionary IS a correct per-process carrier: nothing
%%% leaks between concurrent processes because nothing is shared.
%%%
%%% What it does not do is cross a `spawn` or a message send. A spawned process
%%% starts with an empty dictionary, which is right; it is a different unit of
%%% execution; so the parent span is carried across explicitly, exactly as a
%%% thread boundary is in Ruby and a fiber boundary is in PHP. Inheriting it
%%% silently would be the same mistake as using a frame stack across an await.
-module(execviz).

-export([install/2, install/3, set_domain/1, current/0,
         span_start/2, span_start/3, span_end/2, span_end/3,
         with_span/3, spawn_span/2, gather/2,
         stamp/1, claim/1, release/1, span_event/3,
         lifecycle/2, log/2, flush/0]).

-define(STATE, execviz_state).
-define(SPAN, execviz_span).
-define(BATCH, 200).

%% ========================================================================
%% LIFECYCLE
%% ========================================================================

install(Collector, HostId) -> install(Collector, HostId, <<"app">>).

install(Collector, HostId, Domain) ->
    Pid = case whereis(execviz_buffer) of
        undefined ->
            P = spawn(fun() -> buffer_loop(Collector, HostId, [], #{}) end),
            register(execviz_buffer, P),
            P;
        Existing -> Existing
    end,
    put(?STATE, #{collector => Collector, host => HostId, domain => Domain,
                  trace => sid(), buffer => Pid}),
    Pid.

set_domain(D) ->
    S = state(), put(?STATE, S#{domain => D}), ok.

state() ->
    case get(?STATE) of
        undefined ->
            %% a process that was spawned without carrying context still records
            %% accurately rather than crashing: it inherits nothing and reports it
            #{collector => "http://127.0.0.1:8900", host => <<"beam">>,
              domain => <<"unknown">>, trace => sid(), buffer => whereis(execviz_buffer)};
        S -> S
    end.

current() -> case get(?SPAN) of undefined -> null; S -> S end.

%% ========================================================================
%% SPANS
%% ========================================================================

span_start(Name, Kind) -> span_start(Name, Kind, []).

span_start(Name, Kind, Opts) ->
    S = state(),
    Id = sid(),
    Parent = proplists:get_value(parent, Opts, current()),
    Links = proplists:get_value(links, Opts, []),
    Domain = proplists:get_value(domain, Opts, maps:get(domain, S)),
    Span = #{span_id => Id, trace_id => maps:get(trace, S),
             parent_span_id => Parent, links => Links,
             name => to_bin(Name), kind => to_bin(Kind),
             start => now_secs(), 'end' => null, status => <<"running">>,
             lifecycle => [], events => [], origin => <<"semantic">>,
             host_id => maps:get(host, S), domain => Domain,
             attributes => #{pid => to_bin(pid_to_list(self()))}},
    send({span, Span}),
    Id.

span_end(Id, Status) -> span_end(Id, Status, #{}).

span_end(Id, Status, Attrs) ->
    send({finish, Id, now_secs(), to_bin(Status), Attrs}),
    ok.

lifecycle(Id, Type) -> send({lifecycle, Id, now_secs(), to_bin(Type)}), ok.

%% A log line belongs to the span that was running when it was written.
%% Attaches a line to a named span, whatever is current.
%%
%% Exists for the logger handler, which runs in the caller's process
%% and has already resolved the span before calling: passing it explicitly means
%% the handler never has to assume the dictionary is still what it was.
span_event(Span, Level, Msg) ->
    send({event, Span, now_secs(), to_bin(Level), to_bin(Msg)}), ok.

log(Level, Msg) ->
    case current() of
        null -> ok;
        Id -> send({event, Id, now_secs(), to_bin(Level), to_bin(Msg)}), ok
    end.

%% Runs Fun inside a span, with that span active for whatever it reaches.
with_span(Name, Kind, Fun) ->
    Id = span_start(Name, Kind),
    Prev = get(?SPAN),
    put(?SPAN, Id),
    try
        R = Fun(),
        span_end(Id, ok),
        R
    catch
        Class:Reason:Stack ->
            span_end(Id, error, #{error => to_bin(io_lib:format("~p:~p", [Class, Reason])),
                                  stack => to_bin(io_lib:format("~p", [lists:sublist(Stack, 5)]))}),
            erlang:raise(Class, Reason, Stack)
    after
        case Prev of undefined -> erase(?SPAN); _ -> put(?SPAN, Prev) end
    end.

%% A spawn is a crossing. The child inherits the parent span because it is
%% handed it here, not because the runtime shares anything: a spawned process
%% starts with an empty dictionary, and that is correct.
spawn_span(Name, Fun) ->
    S = state(),
    Parent = current(),
    Id = span_start(Name, spawn),
    Pid = spawn(fun() ->
        put(?STATE, S),
        put(?SPAN, Id),
        try Fun(), span_end(Id, ok)
        catch C:R:_ -> span_end(Id, error, #{error => to_bin(io_lib:format("~p:~p",[C,R]))})
        end
    end),
    _ = Parent,
    {Pid, Id}.

%% A fan-in: the join keeps the enclosing scope as its parent and names every
%% child in links. Parenting it to a child would place it outside
%% its own parent in time.
gather(Name, Funs) ->
    Parent = current(),
    Self = self(),
    Ids = [begin
        Id = span_start(io_lib:format("~s[~p]", [Name, I]), call, [{parent, Parent}]),
        spawn(fun() ->
            put(?SPAN, Id),
            R = try Fun(), ok catch C:Rn:_ -> {error, {C, Rn}} end,
            span_end(Id, case R of ok -> ok; _ -> error end),
            Self ! {done, Id}
        end),
        Id
    end || {I, Fun} <- lists:zip(lists:seq(0, length(Funs) - 1), Funs)],
    [receive {done, _} -> ok after 30000 -> timeout end || _ <- Ids],
    Join = span_start(io_lib:format("~s_join", [Name]), call, [{parent, Parent}, {links, Ids}]),
    span_end(Join, ok),
    Ids.

%% ========================================================================
%% CROSSINGS
%% ========================================================================

%% Context stamped onto a message, read back on the far side. A message send is
%% the BEAM's boundary, and it is explicit here because it is explicit there.
stamp(Msg) ->
    S = state(),
    {execviz, maps:get(trace, S), current(), Msg}.

claim({execviz, Trace, Span, Msg}) ->
    S = state(),
    put(?STATE, S#{trace => Trace}),
    case Span of
        null -> ok;
        _ -> lifecycle(Span, claimed), put(?SPAN, Span)
    end,
    {Msg, Span};
claim(Other) -> {Other, null}.

release(null) -> ok;
release(Span) -> lifecycle(Span, released), span_end(Span, ok), erase(?SPAN), ok.

%% ========================================================================
%% DELIVERY
%% ========================================================================

send(Msg) ->
    case whereis(execviz_buffer) of
        undefined -> ok;          %% never crash the program being observed
        P -> P ! Msg, ok
    end.

flush() ->
    case whereis(execviz_buffer) of
        undefined -> ok;
        P -> P ! {flush, self()}, receive {flushed, N} -> N after 10000 -> timeout end
    end.

%% The buffer is one process holding every span, which is the BEAM way of having
%% shared state without a lock. Two-phase writes are applied here, so the far end
%% receives the same span twice and upserts it.
buffer_loop(Collector, Host, Pending, Sent) ->
    receive
        {span, S} ->
            buffer_loop(Collector, Host, [S | Pending], Sent);
        {finish, Id, T, Status, Attrs} ->
            P2 = [case maps:get(span_id, S) of
                      Id -> S#{'end' => T, status => Status,
                               attributes => maps:merge(maps:get(attributes, S), Attrs)};
                      _ -> S
                  end || S <- Pending],
            buffer_loop(Collector, Host, P2, Sent);
        {lifecycle, Id, T, Type} ->
            P2 = [case maps:get(span_id, S) of
                      Id -> S#{lifecycle => maps:get(lifecycle, S) ++ [#{t => T, type => Type}]};
                      _ -> S
                  end || S <- Pending],
            buffer_loop(Collector, Host, P2, Sent);
        {event, Id, T, Level, Msg} ->
            P2 = [case maps:get(span_id, S) of
                      Id -> S#{events => maps:get(events, S) ++ [#{t => T, level => Level, msg => Msg}]};
                      _ -> S
                  end || S <- Pending],
            buffer_loop(Collector, Host, P2, Sent);
        {flush, From} ->
            {N, Sent2} = deliver(Collector, Host, Pending, Sent),
            From ! {flushed, N},
            buffer_loop(Collector, Host, Pending, Sent2)
    after 1000 ->
        {_, Sent2} = deliver(Collector, Host, Pending, Sent),
        buffer_loop(Collector, Host, Pending, Sent2)
    end.

%% A span is re-sent once its second phase lands, and not otherwise.
deliver(_Collector, _Host, [], Sent) -> {0, Sent};
deliver(Collector, Host, Pending, Sent) ->
    Changed = [S || S <- Pending,
        maps:get(phase_key(S), Sent, undefined) =/= phase_val(S)],
    case Changed of
        [] -> {0, Sent};
        _ ->
            Body = json_obj([{host_id, Host}, {spans, [span_json(S) || S <- Changed]}]),
            case post(Collector ++ "/api/ingest", Body) of
                ok ->
                    S2 = lists:foldl(fun(S, Acc) ->
                        maps:put(phase_key(S), phase_val(S), Acc) end, Sent, Changed),
                    {length(Changed), S2};
                _ -> {0, Sent}      %% a failed delivery is retried, never dropped
            end
    end.

phase_key(S) -> maps:get(span_id, S).
phase_val(S) -> {maps:get('end', S), maps:get(status, S)}.

%% ========================================================================
%% PLUMBING
%% ========================================================================

post(Url, Body) ->
    case parse_url(Url) of
        {Host, Port, Path} ->
            case gen_tcp:connect(Host, Port, [binary, {active, false}, {packet, 0}], 5000) of
                {ok, Sock} ->
                    Req = iolist_to_binary([
                        "POST ", Path, " HTTP/1.1\r\nHost: ", Host, "\r\n",
                        "Content-Type: application/json\r\n",
                        "Content-Length: ", integer_to_list(byte_size(Body)), "\r\n",
                        "Connection: close\r\n\r\n", Body]),
                    gen_tcp:send(Sock, Req),
                    Reply = case gen_tcp:recv(Sock, 0, 5000) of
                                {ok, R} -> R;
                                _ -> <<>>
                            end,
                    gen_tcp:close(Sock),
                    report_refusals(Reply),
                    ok;
                _ -> error
            end;
        _ -> error
    end.

%% Reads what the collector said about the batch.
%%
%% It names every span it refused and why. That explanation reached nobody while
%% the reply was discarded and any answer treated as complete success, so an
%% adapter emitting malformed spans went on emitting them with nothing to show
%% its author.
%%
%% Reported once per distinct reason: a bug in an adapter repeats every second,
%% and a message that repeats with it is one nobody reads.
report_refusals(Reply) when is_binary(Reply) ->
    %% Whitespace-tolerant on purpose: assuming a peer formats JSON compactly is
    %% an assumption about someone else's serialiser, and it fails silently,
    %% the reply is read, nothing matches, and no refusal is ever reported.
    case rejected_count(Reply) of
        0 -> ok;
        _ ->
            case binary:match(Reply, <<"\"reasons\"">>) of
                nomatch -> ok;
                {S, _} ->
                    Tail = binary:part(Reply, S, byte_size(Reply) - S),
                    case {binary:match(Tail, <<"[">>), binary:match(Tail, <<"]">>)} of
                        {{O, _}, {C, _}} when C > O ->
                            Inner = binary:part(Tail, O + 1, C - O - 1),
                            lists:foreach(fun report_one/1,
                                          binary:split(Inner, <<"\",">>, [global]));
                        _ -> ok
                    end
            end
    end;
report_refusals(_) -> ok.

%% The count, whatever spacing the peer used after the colon.
rejected_count(Reply) ->
    case binary:match(Reply, <<"\"rejected\"">>) of
        nomatch -> 0;
        {S, L} ->
            Tail = binary:part(Reply, S + L, min(24, byte_size(Reply) - S - L)),
            Digits = [C || <<C>> <= Tail, C >= $0, C =< $9],
            case Digits of
                [] -> 0;
                _ -> list_to_integer(Digits)
            end
    end.

report_one(Raw) ->
    Reason = binary:replace(Raw, <<"\"">>, <<>>, [global]),
    case Reason of
        <<>> -> ok;
        _ ->
            %% the span id changes every time, so key on the explanation itself
            Key = case binary:split(Reason, <<":">>) of
                      [_, Rest] -> Rest;
                      _ -> Reason
                  end,
            Seen = case get(execviz_reported) of undefined -> []; L -> L end,
            case lists:member(Key, Seen) of
                true -> ok;
                false ->
                    put(execviz_reported, [Key | Seen]),
                    io:format(standard_error,
                              "execviz: the collector refused a span; ~s~n"
                              "  (further spans refused for this reason will not be reported again)~n",
                              [Reason])
            end
    end.

parse_url("http://" ++ Rest) ->
    {HostPort, Path} = case string:split(Rest, "/") of
        [HP] -> {HP, "/"};
        [HP, Tail] -> {HP, "/" ++ Tail}
    end,
    case string:split(HostPort, ":") of
        [H, PortStr] -> {H, list_to_integer(PortStr), Path};
        [H] -> {H, 80, Path}
    end;
parse_url(_) -> error.

span_json(S) ->
    json_obj([{span_id, maps:get(span_id, S)}, {trace_id, maps:get(trace_id, S)},
              {parent_span_id, maps:get(parent_span_id, S)},
              {links, maps:get(links, S)}, {name, maps:get(name, S)},
              {kind, maps:get(kind, S)}, {start, maps:get(start, S)},
              {'end', maps:get('end', S)}, {status, maps:get(status, S)},
              {lifecycle, [json_obj([{t, maps:get(t,L)},{type, maps:get(type,L)}]) || L <- maps:get(lifecycle, S)]},
              {events, [json_obj([{t, maps:get(t,E)},{level, maps:get(level,E)},{msg, maps:get(msg,E)}]) || E <- maps:get(events, S)]},
              {origin, maps:get(origin, S)},
              %% which clock stamped this, so skew analysis knows what it compares
              {clock_source, <<"erlang:system_time">>},
              {host_id, maps:get(host_id, S)},
              {domain, maps:get(domain, S)},
              {attributes, json_obj(maps:to_list(maps:get(attributes, S)))}]).

json_obj(KVs) ->
    iolist_to_binary(["{", lists:join(",", [[jkey(K), ":", jval(V)] || {K, V} <- KVs]), "}"]).

jkey(K) -> ["\"", esc(to_bin(K)), "\""].

jval(null) -> "null";
jval(true) -> "true";
jval(false) -> "false";
jval(V) when is_integer(V) -> integer_to_list(V);
jval(V) when is_float(V) -> io_lib:format("~.6f", [V]);
jval(V) when is_binary(V) ->
    case V of
        <<"{", _/binary>> -> V;          %% already-rendered JSON
        _ -> ["\"", esc(V), "\""]
    end;
jval(V) when is_list(V) ->
    case io_lib:printable_list(V) of
        true -> ["\"", esc(to_bin(V)), "\""];
        false -> ["[", lists:join(",", [jval(X) || X <- V]), "]"]
    end;
jval(V) when is_atom(V) -> ["\"", esc(to_bin(V)), "\""];
jval(V) -> ["\"", esc(to_bin(io_lib:format("~p", [V]))), "\""].

esc(B) when is_binary(B) ->
    << <<(case C of
              $" -> <<"\\\"">>;
              $\\ -> <<"\\\\">>;
              $\n -> <<"\\n">>;
              $\r -> <<"\\r">>;
              $\t -> <<"\\t">>;
              _ -> <<C>>
          end)/binary>> || <<C>> <= B >>.

to_bin(B) when is_binary(B) -> B;
to_bin(A) when is_atom(A) -> atom_to_binary(A, utf8);
to_bin(I) when is_integer(I) -> integer_to_binary(I);
to_bin(L) when is_list(L) -> iolist_to_binary(L).

now_secs() -> erlang:system_time(microsecond) / 1000000.

sid() ->
    <<A:48>> = crypto:strong_rand_bytes(6),
    list_to_binary(io_lib:format("~12.16.0b", [A])).
