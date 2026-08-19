package sdr;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.BiConsumer;

/**
 * Node template for the SDR distributed systems labs.
 *
 * A node is a <em>pure event handler</em>: it receives one event, emits its outgoing messages, and
 * announces {@code done}. Same event sequence in, same actions out.
 *
 * Do not use wall-clock time, threads, or unseeded randomness — replay depends on determinism and
 * the harness reports violations as your bug. Your process must never exit.
 */
public class Node {
    public String id;
    public List<String> peers = new ArrayList<>();
    public int n, f;
    public List<String> provided = new ArrayList<>();   // layers the harness stands in for

    private final Map<String, BiConsumer<String, Map<String, Object>>> handlers = new LinkedHashMap<>();
    private final Map<Integer, Runnable> timerCbs = new LinkedHashMap<>();
    private final List<Object> out = new ArrayList<>();
    private int timerId = 0;

    /** Register a handler for one message type. */
    public void on(String type, BiConsumer<String, Map<String, Object>> h) { handlers.put(type, h); }

    public void send(String dest, Map<String, Object> body) {
        Map<String, Object> e = new LinkedHashMap<>();
        e.put("src", id);
        e.put("dest", dest);
        e.put("body", body);
        out.add(e);
    }

    public void broadcast(Map<String, Object> body) {
        for (String p : peers) if (!p.equals(id)) send(p, body);
    }

    /**
     * Fire after {@code after} units of LOGICAL time; returns the timer id.
     *
     * With a callback, only that callback runs — which is what lets several layers hold timers at
     * once. Timers cannot be cancelled: if you re-arm, guard the callback with a generation counter
     * and ignore stale firings.
     */
    public int setTimer(int after, Runnable cb) {
        timerId++;
        if (cb != null) timerCbs.put(timerId, cb);
        send("harness", body("type", "set_timer", "after", after, "timer_id", timerId));
        return timerId;
    }

    /** Report an observable event (deliver / enter_cs / leader / decide / ...). */
    public void observe(Map<String, Object> body) { send("harness", body); }

    public void log(Object... args) {
        StringBuilder b = new StringBuilder();
        for (Object a : args) b.append(a).append(' ');
        System.err.println(b.toString().trim());
    }

    /** Convenience: {@code body("type", "hello", "n", 3)}. */
    public static Map<String, Object> body(Object... kv) {
        Map<String, Object> m = new LinkedHashMap<>();
        for (int i = 0; i + 1 < kv.length; i += 2) m.put((String) kv[i], kv[i + 1]);
        return m;
    }

    public static int asInt(Object v) { return v instanceof Number ? ((Number) v).intValue() : 0; }

    @SuppressWarnings("unchecked")
    public void run() throws Exception {
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in));
        PrintStream w = System.out;
        String line;
        while ((line = in.readLine()) != null) {
            if (line.isBlank()) continue;
            Map<String, Object> m = (Map<String, Object>) Json.parse(line);
            String src = (String) m.get("src");
            Map<String, Object> b = (Map<String, Object>) m.get("body");
            String type = (String) b.get("type");

            if ("init".equals(type)) {
                id = (String) b.get("node_id");
                peers = (List<String>) (List<?>) b.get("node_ids");
                n = asInt(b.get("n"));
                f = asInt(b.get("f"));
                Object p = b.get("provided");
                if (p != null) provided = (List<String>) (List<?>) p;
            }

            if ("timer".equals(type)) {
                Runnable cb = timerCbs.remove(asInt(b.get("timer_id")));
                if (cb != null) cb.run();
                else if (handlers.containsKey("timer")) handlers.get("timer").accept(src, b);
            } else if (handlers.containsKey(type)) {
                handlers.get(type).accept(src, b);
            }

            // `done` must be last: it is the barrier that lets logical time advance.
            send("harness", body("type", "done"));
            for (Object e : out) w.println(Json.write(e));
            w.flush();
            out.clear();
        }
    }
}
