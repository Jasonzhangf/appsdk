#!/bin/sh
# Test-owned process fixture. Native startup keeps the short deadline focused
# on the stalled write, including a child that never reads its standard input.
for argument do
    if [ "$argument" = server.status ]; then
        printf '%s\n' '{"host_id":"host-fixture","instance_id":"instance-fixture"}'
        exit 0
    fi
done
printf '{"pid":%s,"argv":["com.superlogical.terminal.write"],"stdin":""}\n' "$$" >> "$REXPRIMITIVE_LOG_DIR/invocations.jsonl"
exec /bin/sleep 120
