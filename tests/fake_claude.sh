#!/bin/sh
# Stands in for `claude` in tests. Replays $FAKE_FIXTURE to stdout, writes $FAKE_STDERR to stderr,
# then exits with $FAKE_EXIT.
# With $FAKE_GATE set, it waits after the first line until that file exists (at most 10 s, longer than the test timeout),
# so a test can prove that the first event arrives while the process still runs.
# With $FAKE_HANG set, it then starts `sleep 30` in the background, writes its pid to that file and
# waits, like a long tool call. $FAKE_IGNORE_TERM=leader or =grandchild makes that one ignore SIGTERM.
# With $FAKE_ARGV set, it first writes its arguments to that file, one per line.
# With $FAKE_SESSION_PER_RUN set, every session_id in the fixture becomes run-<pid>, so two runs
# give two sessions.
[ -n "$FAKE_ARGV" ] && printf '%s\n' "$@" > "$FAKE_ARGV"
fixture=$FAKE_FIXTURE
if [ -n "$FAKE_SESSION_PER_RUN" ]; then
    fixture=$(mktemp)
    sed "s/\"session_id\":\"[^\"]*\"/\"session_id\":\"run-$$\"/g" "$FAKE_FIXTURE" > "$fixture"
fi
first=1
while IFS= read -r line || [ -n "$line" ]; do
    printf '%s\n' "$line"
    if [ "$first" = 1 ] && [ -n "$FAKE_GATE" ]; then
        first=0
        tries=0
        while [ ! -e "$FAKE_GATE" ] && [ "$tries" -lt 200 ]; do
            sleep 0.05
            tries=$((tries + 1))
        done
    fi
done < "$fixture"
[ -n "$FAKE_SESSION_PER_RUN" ] && rm -f "$fixture"
if [ -n "$FAKE_HANG" ]; then
    [ "$FAKE_IGNORE_TERM" = leader ] && trap '' TERM
    if [ "$FAKE_IGNORE_TERM" = grandchild ]; then
        (trap '' TERM; exec sleep 30) >/dev/null 2>&1 &
    else
        sleep 30 &
    fi
    echo $! > "$FAKE_HANG"
    wait
fi
[ -n "$FAKE_STDERR" ] && printf '%s\n' "$FAKE_STDERR" >&2
exit "${FAKE_EXIT:-0}"
