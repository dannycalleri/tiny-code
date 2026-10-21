#!/bin/sh
# Stands in for `claude` in tests. Replays $FAKE_FIXTURE to stdout, writes $FAKE_STDERR to stderr,
# then exits with $FAKE_EXIT.
# With $FAKE_GATE set, it waits after the first line until that file exists (at most 10 s, longer than the test timeout),
# so a test can prove that the first event arrives while the process still runs.
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
done < "$FAKE_FIXTURE"
[ -n "$FAKE_STDERR" ] && printf '%s\n' "$FAKE_STDERR" >&2
exit "${FAKE_EXIT:-0}"
